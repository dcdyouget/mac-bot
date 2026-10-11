package bot.mac.mobile.feature.memory

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.design.FontScaleToolbar
import bot.mac.mobile.core.design.LocalMobilePalette
import bot.mac.mobile.core.network.NetworkError
import bot.mac.mobile.core.protocol.*
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.resources.*
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
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
    var confirmDelete by remember { mutableStateOf<JsonObject?>(null) }
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

    val entries = state.memoryEntries.filter { it.str("content").contains(query, ignoreCase = true) }
    fun deleteEntry(entry: JsonObject, expectedVersion: Long = entry.long("version")) {
        scope.launch {
            runCatching {
                repository.callOnHost(hostId, "memory.user.delete", buildJsonObject {
                    put("entry_id", entry.str("id"))
                    put("expected_version", expectedVersion)
                })
            }.onSuccess {
                val result = snackbar.showSnackbar(deletedText, undoText, withDismissAction = true)
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
                item { ProfileCard(state.userProfile, onOpenProfileField) }
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
                            chats = state.chats,
                            bots = state.bots,
                            onClick = { onOpenEntry(entry.str("id")) },
                            onOpenSource = onOpenSource,
                            onSwipeDelete = { confirmDelete = entry },
                        )
                    }
                }
                item { Text(stringResource(Res.string.memory_mobile_hint), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
            }
        }
        SnackbarHost(snackbar, Modifier.align(Alignment.BottomCenter))
    }

    confirmDelete?.let { entry ->
        AlertDialog(
            onDismissRequest = { confirmDelete = null },
            title = { Text(stringResource(Res.string.memory_delete_entry)) },
            text = { Text(entry.str("content")) },
            confirmButton = {
                TextButton(onClick = {
                    confirmDelete = null
                    deleteEntry(entry)
                }) { Text(stringResource(Res.string.common_delete), color = MaterialTheme.colorScheme.error) }
            },
            dismissButton = { TextButton(onClick = { confirmDelete = null }) { Text(stringResource(Res.string.common_cancel)) } },
        )
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
private fun ProfileCard(profile: JsonObject, onOpenField: (String) -> Unit) {
    Column(
        Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surface, RoundedCornerShape(16.dp)).padding(14.dp),
        verticalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        Text(stringResource(Res.string.memory_profile_title), style = MaterialTheme.typography.titleMedium)
        ProfileRow(stringResource(Res.string.memory_profile_name), profile.str("name").ifBlank { "—" }) { onOpenField("name") }
        ProfileRow(
            stringResource(Res.string.memory_profile_language),
            profile.str("language").ifBlank { stringResource(Res.string.memory_profile_language_system) },
        ) { onOpenField("language") }
        ProfileRow(stringResource(Res.string.memory_profile_identity), profile.str("identity").ifBlank { "—" }) { onOpenField("identity") }
        ProfileRow(
            stringResource(Res.string.memory_profile_timezone),
            stringResource(Res.string.memory_profile_timezone_value, profile.str("timezone").ifBlank { "—" }),
            enabled = false,
        ) {}
    }
}

@Composable
private fun ProfileRow(label: String, value: String, enabled: Boolean = true, onClick: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().then(if (enabled) Modifier.clickable(onClick = onClick) else Modifier).padding(vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(label, Modifier.width(88.dp), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Text(value, Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium)
        if (enabled) Text("›", color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

@Composable
private fun QuotaCard(quota: JsonObject) {
    val used = quota.long("used_chars").coerceAtLeast(0L)
    val limit = quota.long("limit_chars").coerceAtLeast(1L)
    val fraction = (used.toFloat() / limit.toFloat()).coerceIn(0f, 1f)
    val palette = LocalMobilePalette.current
    Column(Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surface, RoundedCornerShape(14.dp)).padding(14.dp)) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Text(stringResource(Res.string.memory_usage), Modifier.weight(1f), style = MaterialTheme.typography.titleSmall)
            Text(stringResource(Res.string.memory_usage_value, used, limit), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Box(Modifier.fillMaxWidth().padding(top = 10.dp).height(6.dp).background(MaterialTheme.colorScheme.surfaceVariant, RoundedCornerShape(3.dp))) {
            Box(Modifier.fillMaxWidth(fraction).fillMaxHeight().background(palette.accent, RoundedCornerShape(3.dp)))
        }
        Text(stringResource(Res.string.memory_usage_hint), Modifier.padding(top = 8.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

@Composable
private fun MemoryEntryRow(
    entry: JsonObject,
    chats: List<JsonObject>,
    bots: List<JsonObject>,
    onClick: () -> Unit,
    onOpenSource: (chatId: String, messageId: String) -> Unit,
    onSwipeDelete: () -> Unit,
) {
    var offsetX by remember(entry.str("id")) { mutableStateOf(0f) }
    val source = entry.obj("source")
    val sender = source.obj("by")
    val chatTitle = chats.firstOrNull { it.str("id") == source.str("chat_id") }?.str("title")
        ?: chats.firstOrNull { it.str("id") == source.str("chat_id") }?.str("name").orEmpty()
    val botName = bots.firstOrNull { it.str("id") == sender.str("bot_id") }?.str("name") ?: sender.str("bot_id")
    val date = entry.str("created_at").substringBefore('T').ifBlank { entry.str("created_at") }
    val sourceLabel = when (sender.str("kind")) {
        "user" -> stringResource(Res.string.memory_source_user)
        "system" -> stringResource(Res.string.memory_source_curator)
        "bot" -> stringResource(Res.string.memory_source_bot, botName)
        else -> stringResource(Res.string.memory_source_curator)
    }
    val detail = when {
        sender.str("kind") == "user" -> date
        chatTitle.isNotBlank() -> stringResource(Res.string.memory_source_detail, chatTitle, date)
        else -> date
    }
    val sourceChatId = source.str("chat_id")
    val sourceMessageId = source.str("message_id")
    Row(
        Modifier.fillMaxWidth()
            .offset { IntOffset(offsetX.roundToInt(), 0) }
            .pointerInput(entry.str("id")) {
                var total = 0f
                detectHorizontalDragGestures(
                    onDragStart = { total = 0f },
                    onHorizontalDrag = { change, amount ->
                        change.consume()
                        total += amount
                        offsetX = (offsetX + amount).coerceIn(-140f, 0f)
                    },
                    onDragEnd = {
                        if (total < -64f) onSwipeDelete() else offsetX = 0f
                    },
                    onDragCancel = { offsetX = 0f },
                )
            }
            .background(MaterialTheme.colorScheme.surface, RoundedCornerShape(14.dp))
            .clickable(onClick = onClick)
            .padding(horizontal = 14.dp, vertical = 12.dp),
    ) {
        Column(Modifier.weight(1f)) {
            Text(entry.str("content"), style = MaterialTheme.typography.bodyLarge, lineHeight = MaterialTheme.typography.bodyLarge.lineHeight)
            Row(Modifier.fillMaxWidth().padding(top = 7.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(sourceLabel, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                if (detail.isNotBlank()) {
                    Text(" · $detail", style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1)
                }
                if (sourceChatId.isNotBlank() && sourceMessageId.isNotBlank()) {
                    Text(
                        stringResource(Res.string.memory_source_open),
                        Modifier.padding(start = 8.dp).clickable { onOpenSource(sourceChatId, sourceMessageId) },
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.primary,
                        maxLines = 1,
                    )
                }
            }
        }
        Text("›", Modifier.padding(start = 8.dp).align(Alignment.CenterVertically), color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

@Composable
fun MemoryEntryEditorScreen(repository: MobileRepository, entryId: String, onBack: () -> Unit, onOpenSource: (chatId: String, messageId: String) -> Unit) {
    val state by repository.state.collectAsState()
    val hostId = repository.activeHost.collectAsState().value?.id.orEmpty()
    val scope = rememberCoroutineScope()
    val existing = state.memoryEntries.firstOrNull { it.str("id") == entryId }
    var content by rememberSaveable(hostId, entryId) { mutableStateOf("") }
    var baseVersion by rememberSaveable(hostId, entryId) { mutableStateOf(0L) }
    var entryLoaded by rememberSaveable(hostId, entryId) { mutableStateOf(entryId.isBlank()) }
    var error by remember { mutableStateOf<String?>(null) }
    var conflict by remember { mutableStateOf<JsonObject?>(null) }
    var deleteConflict by remember { mutableStateOf<JsonObject?>(null) }
    var deleting by remember { mutableStateOf(false) }
    val loadingError = stringResource(Res.string.feature_loading_error)
    val quotaError = stringResource(Res.string.memory_quota_full)
    val conflictError = stringResource(Res.string.memory_conflict_simple)
    val conflictDeleteMine = stringResource(Res.string.memory_conflict_delete_mine)
    val conflictKeepLatest = stringResource(Res.string.memory_conflict_keep_latest)
    LaunchedEffect(hostId, entryId) {
        if (entryId.isNotBlank()) {
            runCatching {
                val result = repository.callOnHost(hostId, "memory.user.list")
                val current = result.objects("entries").firstOrNull { it.str("id") == entryId }
                if (current != null) {
                    content = current.str("content")
                    baseVersion = current.long("version")
                    entryLoaded = true
                } else error = loadingError
            }.onFailure { error = loadingError }
        }
    }
    val isNew = entryId.isBlank()
    fun save(expectedVersion: Long = baseVersion) {
        scope.launch {
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
        }
    }
    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background).imePadding()) {
        FontScaleToolbar {
            Row(Modifier.fillMaxWidth().heightIn(min = 64.dp).padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                TextButton(onClick = onBack) { Text(stringResource(Res.string.common_back)) }
                Text(stringResource(if (isNew) Res.string.memory_add_title else Res.string.memory_edit_title), Modifier.weight(1f), style = MaterialTheme.typography.titleLarge)
                TextButton(onClick = { save() }, enabled = entryLoaded && content.isNotBlank() && content.length <= 500) { Text("✓", style = MaterialTheme.typography.titleLarge) }
            }
        }
        Column(Modifier.weight(1f).fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp)) {
            OutlinedTextField(
                value = content,
                onValueChange = { if (it.length <= 500) content = it },
                modifier = Modifier.fillMaxWidth().weight(1f),
                minLines = 6,
                maxLines = 18,
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences),
            )
            Text(stringResource(Res.string.memory_edit_count, content.length), Modifier.fillMaxWidth().padding(vertical = 8.dp), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
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
                Button(onClick = { deleting = true }, modifier = Modifier.fillMaxWidth().padding(top = 18.dp), colors = ButtonDefaults.buttonColors(containerColor = MaterialTheme.colorScheme.errorContainer, contentColor = MaterialTheme.colorScheme.onErrorContainer)) {
                    Text(stringResource(Res.string.memory_delete_entry))
                }
            }
            error?.let { Text(it, Modifier.padding(top = 8.dp), color = MaterialTheme.colorScheme.error) }
        }
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
                        runCatching { repository.callOnHost(hostId, "memory.user.delete", buildJsonObject { put("entry_id", entryId); put("expected_version", baseVersion) }) }
                            .onSuccess { onBack() }
                            .onFailure { failure ->
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
            "bot" -> stringResource(Res.string.memory_source_bot, remote.obj("updated_by").str("bot_id"))
            else -> stringResource(Res.string.memory_source_curator)
        }
        val conflictText = stringResource(Res.string.memory_conflict, updatedBy, remote.long("version"))
        AlertDialog(
            onDismissRequest = { conflict = null },
            title = { Text(stringResource(Res.string.memory_conflict_title)) },
            text = { Column(verticalArrangement = Arrangement.spacedBy(8.dp)) { Text(conflictText); Text(remote.str("content"), style = MaterialTheme.typography.bodyMedium) } },
            confirmButton = { TextButton(onClick = { val version = remote.long("version"); conflict = null; save(version) }) { Text(stringResource(Res.string.memory_conflict_mine)) } },
            dismissButton = { TextButton(onClick = { content = remote.str("content"); baseVersion = remote.long("version"); conflict = null }) { Text(stringResource(Res.string.memory_conflict_theirs)) } },
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
                    if (it.length <= maxLength) {
                        value = it
                        if (field == "language") useSystem = false
                    }
                },
                modifier = Modifier.fillMaxWidth(),
                label = { Text(stringResource(when (field) { "name" -> Res.string.memory_profile_name_hint; "identity" -> Res.string.memory_profile_identity_hint; else -> Res.string.memory_profile_language })) },
                minLines = if (field == "identity") 3 else 1,
                maxLines = if (field == "identity") 4 else 1,
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences),
            )
            if (field == "language") {
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                    Checkbox(checked = useSystem, onCheckedChange = { useSystem = it })
                    Text(stringResource(Res.string.memory_profile_language_system), Modifier.clickable { useSystem = !useSystem })
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
                Text(remote.str(field).ifBlank { stringResource(Res.string.memory_profile_language_system) })
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
    val date = entry.str("created_at").substringBefore('T')
    val origin = when (sender.str("kind")) {
        "user" -> stringResource(Res.string.memory_source_user)
        "bot" -> stringResource(Res.string.memory_source_bot, bots.firstOrNull { it.str("id") == sender.str("bot_id") }?.str("name") ?: sender.str("bot_id"))
        else -> stringResource(Res.string.memory_source_curator)
    }
    return if (title.isBlank()) "$origin · $date" else stringResource(Res.string.memory_source_detail_with_origin, origin, title, date)
}
