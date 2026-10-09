package bot.mac.mobile.feature.chat

import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.snapshotFlow
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.protocol.arr
import bot.mac.mobile.core.protocol.boolean
import bot.mac.mobile.core.protocol.obj
import bot.mac.mobile.core.protocol.objects
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.platform.PickedFile
import bot.mac.mobile.core.platform.exportFile
import bot.mac.mobile.core.platform.platformFilePicker
import bot.mac.mobile.core.platform.platformScreenImageDecoder
import bot.mac.mobile.core.platform.platformPersistentStore
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.core.state.MobileState
import bot.mac.mobile.core.ui.MarkdownText
import bot.mac.mobile.resources.*
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.jetbrains.compose.resources.stringResource

@Composable
fun ChatScreen(
    repository: MobileRepository,
    chatId: String,
    onOpenTrace: (assignmentId: String?, chatId: String?) -> Unit = { _, _ -> },
    onOpenProject: (String) -> Unit = {},
    onBack: () -> Unit = {},
    onProjectAction: (projectId: String, action: String) -> Unit = { _, _ -> },
    onOpenScreen: (botId: String, tabId: String?) -> Unit = { _, _ -> },
    onOpenChat: (chatId: String) -> Unit = {},
    onLoopAction: (rootMessageId: String, action: String) -> Unit = { _, _ -> },
    onTakeover: (botId: String) -> Unit = {},
    onOpenArtifact: (artifactId: String, pathOrUrl: String, projectId: String?) -> Unit = { _, _, _ -> },
    onOpenHistory: (chatId: String) -> Unit = {},
    artifactProjectId: String? = null,
    showHeader: Boolean = true,
) {
    val state by repository.state.collectAsState()
    val messages = state.messages[chatId].orEmpty()
    val chat = state.chats.firstOrNull { it.str("id") == chatId }
    var draft by remember(chatId) { mutableStateOf("") }
    var replyTo by remember(chatId) { mutableStateOf<String?>(null) }
    var mentionBots by remember(chatId) { mutableStateOf(emptySet<String>()) }
    var attachments by remember(chatId) { mutableStateOf(emptyList<String>()) }
    var attachmentNames by remember(chatId) { mutableStateOf(emptyList<String>()) }
    var error by remember(chatId) { mutableStateOf<String?>(null) }
    var filePreviews by remember(chatId) { mutableStateOf(emptyMap<String, String>()) }
    var imagePreviews by remember(chatId) { mutableStateOf(emptyMap<String, ImageBitmap>()) }
    var draftLoaded by remember(chatId) { mutableStateOf(false) }
    var historyHasMore by remember(chatId) { mutableStateOf(false) }
    var historyBeforeSeq by remember(chatId) { mutableStateOf<Long?>(null) }
    var loadingOlder by remember(chatId) { mutableStateOf(false) }
    var pendingAnchor by remember(chatId) { mutableStateOf<ScrollAnchor?>(null) }
    var threadRoot by remember(chatId) { mutableStateOf<JsonObject?>(null) }
    var threadReplies by remember(chatId) { mutableStateOf(emptyList<JsonObject>()) }
    var threadLoading by remember(chatId) { mutableStateOf(false) }
    var stickToBottom by remember(chatId) { mutableStateOf(true) }
    val listState = rememberLazyListState()
    val scope = rememberCoroutineScope()
    val hostId = repository.activeHost.collectAsState().value?.id ?: "active"
    val readOnly = chat?.str("kind") == "bot_dm"
    val memberIds = chat?.arr("member_bot_ids").orEmpty().map { it.toString().trim('"') }
    val mentionCandidates = state.bots.filter { it.str("id") in memberIds || memberIds.isEmpty() }
    val groupChat = chat?.str("kind") == "project"
    val loadingError = stringResource(Res.string.feature_loading_error)
    val sendError = stringResource(Res.string.feature_send_error)
    LaunchedEffect(hostId, chatId) {
        draftLoaded = false
        draft = runCatching { platformPersistentStore().read("draft:$hostId:$chatId") }.getOrNull().orEmpty()
        draftLoaded = true
    }
    LaunchedEffect(draft, hostId, chatId, draftLoaded) {
        if (draftLoaded) runCatching { platformPersistentStore().write("draft:$hostId:$chatId", draft) }
    }

    LaunchedEffect(chatId) {
        runCatching {
            repository.call("chat.history", buildJsonObject { put("chat_id", chatId); put("limit", 100) })

        }.onSuccess { result ->
            historyHasMore = result.boolean("has_more")
            historyBeforeSeq = result.objects("messages").minOfOrNull { it.longValue("seq") ?: Long.MAX_VALUE }
                ?.takeIf { it != Long.MAX_VALUE }
        }.onFailure { error = it.message ?: loadingError }
    }

    LaunchedEffect(messages.size, messages.lastOrNull()?.toString(), pendingAnchor) {
        pendingAnchor?.let { anchor ->
            if (messages.firstOrNull()?.str("id") != anchor.id) {
                val index = messages.indexOfFirst { it.str("id") == anchor.id }
                if (index >= 0) {
                    withFrameNanos { }
                    listState.scrollToItem(index + if (historyHasMore) 1 else 0, anchor.offset)
                }
                pendingAnchor = null
            }
        }
    }

    LaunchedEffect(chatId, historyHasMore) {
        snapshotFlow {
            val lastVisible = listState.layoutInfo.visibleItemsInfo.lastOrNull()?.index
            val lastIndex = listState.layoutInfo.totalItemsCount - 1
            lastVisible == null || lastIndex < 0 || lastVisible >= lastIndex - 1
        }.collect { stickToBottom = it }
    }

    LaunchedEffect(messages.size, messages.lastOrNull()?.toString(), historyHasMore) {
        if (messages.isEmpty() || !stickToBottom) return@LaunchedEffect
        val lastIndex = messages.lastIndex + if (historyHasMore) 1 else 0
        listState.scrollToItem(lastIndex)
    }

    LaunchedEffect(chatId, chat?.longValue("last_seq")) {
        val seq=chat?.longValue("last_seq") ?: return@LaunchedEffect
        runCatching { repository.call("chat.mark_read",buildJsonObject { put("chat_id",chatId);put("seq",seq) }) }
    }
    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        if (showHeader) {
            Row(
                Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                IconButton(onClick = onBack) { Text(stringResource(Res.string.feature_back), style = MaterialTheme.typography.headlineSmall) }
                Column(Modifier.weight(1f)) {
                    Text(chat?.str("title").orEmpty(), style = MaterialTheme.typography.titleLarge)
                    chat?.str("attention")?.takeIf { it.isNotBlank() }?.let { StatusLabel(it) }
                }
                chat?.str("project_id")?.takeIf { it.isNotBlank() }?.let { projectId ->
                    Text("ⓘ", Modifier.padding(8.dp))
                    IconButton(onClick = { onOpenProject(projectId) }) { Text("›") }
                }
                IconButton(onClick = { onOpenHistory(chatId) }) { Text("↺") }
            }
            HorizontalDivider()
        }
        LazyColumn(
            state = listState,
            modifier = Modifier.weight(1f).fillMaxWidth().padding(horizontal = 12.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            if (historyHasMore) {
                item(key = "load-older") {
                    Button(
                        enabled = !loadingOlder,
                        onClick = {
                            val beforeSeq = historyBeforeSeq ?: messages.minOfOrNull { it.longValue("seq") ?: Long.MAX_VALUE }
                                ?.takeIf { it != Long.MAX_VALUE }
                            if (beforeSeq != null) {
                                val anchor = messages.firstOrNull()?.str("id")?.takeIf { it.isNotBlank() }?.let {
                                    ScrollAnchor(it, listState.firstVisibleItemScrollOffset)
                                }
                                loadingOlder = true
                                scope.launch {
                                    runCatching {
                                        repository.call("chat.history", buildJsonObject {
                                            put("chat_id", chatId)
                                            put("before_seq", beforeSeq)
                                            put("limit", 100)
                                        })
                                    }.onSuccess { result ->
                                        historyHasMore = result.boolean("has_more")
                                        historyBeforeSeq = result.objects("messages").minOfOrNull { it.longValue("seq") ?: Long.MAX_VALUE }
                                            ?.takeIf { it != Long.MAX_VALUE }
                                        val existingIds = messages.asSequence().map { it.str("id") }.toSet()
                                        pendingAnchor = if (result.objects("messages").any { it.str("id") !in existingIds }) anchor else null
                                    }.onFailure { error = it.message ?: loadingError }
                                    loadingOlder = false
                                }
                            }
                        },
                        modifier = Modifier.fillMaxWidth(),
                    ) { Text(if (loadingOlder) stringResource(Res.string.feature_loading) else stringResource(Res.string.feature_load_older)) }
                }
            }
            items(messages, key = { it.str("id").ifBlank { "message:${it.hashCode()}" } }) { message ->
                ChatMessageRow(
                    message = message,
                    onReply = { replyTo = it },
                    onOpenThread = { rootId ->
                        threadLoading = true
                        scope.launch {
                            runCatching {
                                repository.call("chat.thread", buildJsonObject { put("chat_id", chatId); put("root_message_id", rootId) })
                            }.onSuccess { result ->
                                threadRoot = result.obj("root").takeIf { it.str("id").isNotBlank() }
                                threadReplies = result.objects("replies")
                            }.onFailure { error = it.message ?: loadingError }
                            threadLoading = false
                        }
                    },
                    onReact = { id, emoji -> scope.launch { runCatching { repository.call("chat.react", buildJsonObject { put("message_id", id); put("emoji", emoji); put("on", true) }) }.onFailure { error = it.message ?: sendError } } },
                    onOpenTrace = onOpenTrace,
                    onApproval = { id, decision -> scope.launch { runCatching { repository.call("approval.decide", buildJsonObject { put("approval_id", id); put("decision", decision) }) }.onFailure { error = it.message ?: sendError } } },
                    onQuestion = { id, index, text -> scope.launch { runCatching { repository.call("question.answer", buildJsonObject {
                        put("question_id", id)
                        index?.let { put("option_index", it) }
                        text?.let { put("text", it) }
                    }) }.onFailure { error = it.message ?: sendError } } },
                    onPreviewFile = { file ->
                        scope.launch {
                            val ref = fileRef(file)
                            val params = fileParams(ref)
                            runCatching {
                                if (ref.str("mime").startsWith("image/")) {
                                    val image = platformScreenImageDecoder().decodeJpeg(repository.fetchBytes("/api/v1/files", params))
                                        ?: throw IllegalStateException("Unable to decode image")
                                    imagePreviews = imagePreviews + (fileKey(ref) to image)
                                } else {
                                    filePreviews = filePreviews + (fileKey(ref) to repository.fetchText("/api/v1/files", params))
                                }
                            }.onFailure { error = it.message ?: sendError }
                        }
                    },
                    onExportFile = { file ->
                        scope.launch {
                            val ref = fileRef(file)
                            runCatching { repository.fetchBytes("/api/v1/files", fileParams(ref)) }
                                .mapCatching { bytes -> check(exportFile(PickedFile(ref.str("name").ifBlank { "download" }, ref.str("mime"), bytes))) { "Unable to export file" } }
                                .onFailure { error = it.message ?: sendError }
                        }
                    },
                    filePreviews = filePreviews,
                    imagePreviews = imagePreviews,
                    onOpenProject = onOpenProject,
                    onProjectAction = onProjectAction,
                    onOpenScreen = onOpenScreen,
                    onOpenChat = onOpenChat,
                    onLoopAction = onLoopAction,
                    onTakeover = onTakeover,
                    onOpenArtifact = onOpenArtifact,
                    artifactProjectId = artifactProjectId ?: chat?.str("project_id")?.takeIf { it.isNotBlank() },
                state = state,
                )
            }
        }
        if (threadLoading) {
            Text(stringResource(Res.string.feature_thread_loading), Modifier.padding(horizontal = 12.dp), style = MaterialTheme.typography.labelSmall)
        }
        threadRoot?.let { root ->
            ThreadPanel(
                root = root,
                replies = threadReplies,
                state = state,
                onReply = { messageId -> replyTo = messageId; threadRoot = null },
                onClose = { threadRoot = null; threadReplies = emptyList() },
            )
        }
        replyTo?.let { id ->
            Surface(Modifier.fillMaxWidth(), color = MaterialTheme.colorScheme.secondaryContainer) {
                Row(Modifier.padding(horizontal = 16.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text(stringResource(Res.string.feature_reply_message, id), Modifier.weight(1f), style = MaterialTheme.typography.labelMedium)
                    IconButton(onClick = { replyTo = null }) { Text("×") }
                }
            }
        }
        if (mentionCandidates.isNotEmpty() || groupChat) {
            Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 10.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                if (groupChat) {
                    FilterChip(
                        selected = mentionBots.contains("__everyone__"),
                        onClick = { mentionBots = if (mentionBots.contains("__everyone__")) mentionBots - "__everyone__" else mentionBots + "__everyone__" },
                        label = { Text(stringResource(Res.string.feature_mention_everyone)) },
                    )
                }
                mentionCandidates.forEach { bot ->
                    val id = bot.str("id")
                    FilterChip(
                        selected = mentionBots.contains(id),
                        onClick = { mentionBots = if (mentionBots.contains(id)) mentionBots - id else mentionBots + id },
                        label = { Text("@${bot.str("name")}") },
                    )
                }
            }
        }
        error?.let { Text(it, Modifier.padding(horizontal = 12.dp), color = MaterialTheme.colorScheme.error) }
        attachmentNames.forEach { name -> Text(stringResource(Res.string.feature_attachment_added, name), Modifier.padding(horizontal = 12.dp), style = MaterialTheme.typography.labelSmall) }
        Row(Modifier.fillMaxWidth().padding(10.dp), verticalAlignment = Alignment.Bottom) {
            IconButton(enabled = !readOnly, onClick = {
                scope.launch {
                    runCatching { platformFilePicker().pickFile() }.getOrNull()?.let { picked ->
                        runCatching { repository.uploadFile(picked) }
                            .onSuccess { upload ->
                                val uploadId = upload.str("upload_id")
                                if (uploadId.isNotBlank()) { attachments = attachments + uploadId; attachmentNames = attachmentNames + picked.name }
                            }
                            .onFailure { error = it.message ?: sendError }
                    }
                }
            }) { Text("＋") }
            OutlinedTextField(
                value = draft,
                onValueChange = { draft = it },
                modifier = Modifier.weight(1f),
                placeholder = { Text(stringResource(Res.string.feature_send)) },
                maxLines = 4,
            )
            Spacer(Modifier.padding(4.dp))
            Button(
                enabled = draft.isNotBlank() && !readOnly,
                onClick = {
                    val text = draft.trim()
                    scope.launch {
                        runCatching {
                            repository.call("chat.send", buildJsonObject {
                                put("chat_id", chatId)
                                put("text", text)
                                put("mentions", buildJsonArray {
                                    mentionBots.forEach { botId ->
                                        if (botId == "__everyone__") add(buildJsonObject { put("kind", "everyone") })
                                        else add(buildJsonObject { put("kind", "bot"); put("bot_id", botId); put("instruction", JsonNull) })
                                    }
                                })
                                put("attachments", buildJsonArray { attachments.forEach { add(JsonPrimitive(it)) } })
                                replyTo?.let { put("reply_to", it) }
                            })
                        }.onSuccess {
                            draft = ""
                            replyTo = null
                            mentionBots = emptySet()
                            attachments = emptyList()
                            attachmentNames = emptyList()
                            error = null
                        }.onFailure { error = it.message ?: sendError }
                    }
                },
            ) { Text(if (readOnly) stringResource(Res.string.feature_readonly) else stringResource(Res.string.feature_send)) }
        }
    }
}

private data class ScrollAnchor(val id: String, val offset: Int)

@Composable
private fun ThreadPanel(
    root: JsonObject,
    replies: List<JsonObject>,
    state: MobileState,
    onReply: (String) -> Unit,
    onClose: () -> Unit,
) {
    Surface(Modifier.fillMaxWidth().padding(horizontal = 12.dp), color = MaterialTheme.colorScheme.secondaryContainer) {
        Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                Text(stringResource(Res.string.feature_thread_title), Modifier.weight(1f), style = MaterialTheme.typography.titleMedium)
                IconButton(onClick = onClose) { Text("×") }
            }
            Text(stringResource(Res.string.feature_thread_root), style = MaterialTheme.typography.labelMedium)
            ThreadMessageCard(root, state, onReply)
            Text(stringResource(Res.string.feature_thread_replies), style = MaterialTheme.typography.labelMedium)
            if (replies.isEmpty()) Text(stringResource(Res.string.feature_thread_empty), style = MaterialTheme.typography.bodySmall)
            replies.forEach { reply -> ThreadMessageCard(reply, state, onReply) }
        }
    }
}

@Composable
private fun ThreadMessageCard(message: JsonObject, state: MobileState, onReply: (String) -> Unit) {
    val body = message.str("fallback_text").ifBlank {
        message.arr("blocks").mapNotNull { (it as? JsonObject)?.str("markdown")?.takeIf { text -> text.isNotBlank() } }
            .joinToString("\n")
    }
    Column(Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surface).padding(8.dp)) {
        val senderId = message.obj("sender").str("bot_id")
        val sender = state.bots.firstOrNull { it.str("id") == senderId }?.str("name").orEmpty()
            .ifBlank { if (senderId.isBlank()) stringResource(Res.string.feature_bot) else senderId }
        if (sender.isNotBlank()) Text(sender, style = MaterialTheme.typography.labelSmall)
        if (body.isNotBlank()) MarkdownText(body, Modifier.fillMaxWidth())
        val id = message.str("id")
        if (id.isNotBlank()) Button(onClick = { onReply(id) }) { Text(stringResource(Res.string.feature_reply)) }
    }
}

@Composable
internal fun ChatMessageRow(
    message: JsonObject,
    state: MobileState,
    onReply: (String) -> Unit,
    onOpenThread: (String) -> Unit,
    onReact: (String, String) -> Unit,
    onOpenTrace: (String?, String?) -> Unit,
    onApproval: (String, String) -> Unit,
    onQuestion: (String, Int?, String?) -> Unit,
    onPreviewFile: (JsonObject) -> Unit,
    onExportFile: (JsonObject) -> Unit,
    filePreviews: Map<String, String>,
    imagePreviews: Map<String, ImageBitmap>,
    onOpenProject: (String) -> Unit,
    onProjectAction: (String, String) -> Unit,
    onOpenScreen: (String, String?) -> Unit,
    onOpenChat: (String) -> Unit,
    onLoopAction: (String, String) -> Unit,
    onTakeover: (String) -> Unit,
    onOpenArtifact: (String, String, String?) -> Unit,
    artifactProjectId: String?,
) {
    val sender = message.obj("sender")
    val isUser = sender.str("kind") == "user"
    val id = message.str("id")
    val blocks = message.arr("blocks")
    val assignmentId = message.str("assignment_id")
    val senderId = sender.str("bot_id")
    val senderLabel = state.bots.firstOrNull { it.str("id")==senderId }?.str("name")?.takeIf { it.isNotBlank() } ?: if (senderId.isBlank()) stringResource(Res.string.feature_bot) else senderId
    Column(Modifier.fillMaxWidth(), horizontalAlignment = if (isUser) Alignment.End else Alignment.Start) {
        Text(if (isUser) stringResource(Res.string.feature_you) else senderLabel, style = MaterialTheme.typography.labelSmall)
        Surface(
            shape = RoundedCornerShape(16.dp),
            color = if (isUser) MaterialTheme.colorScheme.primaryContainer else MaterialTheme.colorScheme.surfaceVariant,
        ) {
            Column(Modifier.padding(12.dp).fillMaxWidth(0.9f)) {
                if (message.boolean("streaming")) Text(stringResource(Res.string.feature_typing), style = MaterialTheme.typography.labelMedium)
                val fallback = message.str("fallback_text")
                val unknown = blocks.any { element ->
                    val block = element as? JsonObject ?: return@any true
                    block.str("type") !in KnownBlockTypes
                }
                if (blocks.isEmpty() || unknown) if (fallback.isNotBlank()) Text(fallback, style = MaterialTheme.typography.bodyLarge)
                blocks.forEach { element ->
                    (element as? JsonObject)?.let { block ->
                        BlockView(block, state, onApproval, onQuestion, onPreviewFile, onExportFile, filePreviews, imagePreviews,
                            onOpenTrace, onOpenProject, onProjectAction, onOpenScreen, onOpenChat, onLoopAction, onTakeover, onOpenArtifact, artifactProjectId)
                    }
                }
                if (assignmentId.isNotBlank()) {
                    Text(stringResource(Res.string.feature_trace_running), Modifier.padding(top = 8.dp), color = MaterialTheme.colorScheme.primary, style = MaterialTheme.typography.labelLarge)
                    Button(onClick = { onOpenTrace(assignmentId, null) }) { Text(stringResource(Res.string.feature_detail)) }
                }
                Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                    Text("↩", Modifier.padding(4.dp))
                    Text("☺", Modifier.padding(4.dp))
                    if (id.isNotBlank()) {
                        IconButton(onClick = { onReply(id) }) { Text(stringResource(Res.string.feature_reply)) }
                        IconButton(onClick = { onReact(id, "👍") }) { Text("👍") }
                    }
                    val threadCount = message.longValue("thread_count") ?: 0L
                    if (threadCount > 0 && id.isNotBlank()) {
                        Button(onClick = { onOpenThread(id) }) {
                            Text(stringResource(Res.string.feature_thread_open, threadCount.toInt()))
                        }
                    }
                }
                DeliveryView(message.arr("delivery"), state)
            }
        }
    }
}

private val KnownBlockTypes = setOf(
    "text", "image", "file", "task_card", "completion", "progress", "blocked", "question", "project_card", "review_card",
    "delegation", "approval", "approval_ref", "takeover_request", "bot_dm_ref", "system", "loop_paused",
)

@Composable
private fun BlockView(
    block: JsonObject,
    state: MobileState,
    onApproval: (String, String) -> Unit,
    onQuestion: (String, Int?, String?) -> Unit,
    onPreviewFile: (JsonObject) -> Unit,
    onExportFile: (JsonObject) -> Unit,
    filePreviews: Map<String, String>,
    imagePreviews: Map<String, ImageBitmap>,
    onOpenTrace: (String?, String?) -> Unit,
    onOpenProject: (String) -> Unit,
    onProjectAction: (String, String) -> Unit,
    onOpenScreen: (String, String?) -> Unit,
    onOpenChat: (String) -> Unit,
    onLoopAction: (String, String) -> Unit,
    onTakeover: (String) -> Unit,
    onOpenArtifact: (String, String, String?) -> Unit,
    artifactProjectId: String?,
) {
    val kind = block.str("type")
    when (kind) {
        "text" -> MarkdownText(block.str("markdown"), Modifier.fillMaxWidth())
        "image" -> {
            val file = fileRef(block)
            val key = fileKey(file)
            imagePreviews[key]?.let { Image(it, contentDescription = file.str("name"), modifier = Modifier.fillMaxWidth().heightIn(max = 260.dp)) }
            if (file.str("path").isNotBlank()) {
                Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    Button(onClick = { onPreviewFile(file) }) { Text(stringResource(Res.string.feature_preview_file)) }
                    Button(onClick = { onExportFile(file) }) { Text(stringResource(Res.string.feature_export_file)) }
                }
            } else Text(file.str("name"), style = MaterialTheme.typography.bodyMedium)
        }
        "file" -> {
            val file = fileRef(block)
            val key = fileKey(file)
            Text(file.str("name").ifBlank { file.str("path") }, style = MaterialTheme.typography.bodyMedium)
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Button(onClick = { onPreviewFile(file) }) { Text(stringResource(Res.string.feature_preview_file)) }
                Button(onClick = { onExportFile(file) }) { Text(stringResource(Res.string.feature_export_file)) }
            }
            filePreviews[key]?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
        }
        "task_card" -> {
            val assignmentId = block.str("assignment_id")
            val assignment = state.assignments.firstOrNull { it.str("id") == assignmentId }
            val assignmentTitle = assignment?.str("title").orEmpty()
            val assignmentLabel = if (assignmentTitle.isBlank()) stringResource(Res.string.feature_task) else assignmentTitle
            Text("⟳ $assignmentLabel", style = MaterialTheme.typography.titleSmall)
            assignment?.str("status")?.takeIf { it.isNotBlank() }?.let { StatusLabel(it) }
            if (assignmentId.isNotBlank()) Button(onClick = { onOpenTrace(assignmentId, null) }) { Text(stringResource(Res.string.feature_detail)) }
        }
        "completion" -> {
            MarkdownText("✓ ${block.str("summary")}", Modifier.fillMaxWidth())
            block.arr("next").forEach { element ->
                val handoff = element as? JsonObject ?: return@forEach
                val botId = handoff.str("bot_id")
                val botName = state.bots.firstOrNull { it.str("id") == botId }?.str("name").orEmpty().ifBlank { botId }
                Text(stringResource(Res.string.feature_completion_next, botName), style = MaterialTheme.typography.titleSmall)
                MarkdownText(handoff.str("instruction"), Modifier.fillMaxWidth())
            }
            block.arr("artifacts").forEach { element ->
                val artifact = element as? JsonObject ?: return@forEach
                val url = artifact.str("path_or_url")
                Button(onClick = { onOpenArtifact(artifact.str("artifact_id"), url, artifactProjectId) }) { Text("▤ ${artifact.str("title").ifBlank { url }}") }
            }
        }
        "progress" -> Text(stringResource(Res.string.feature_progress, block.str("text")), style = MaterialTheme.typography.bodyMedium)
        "blocked" -> Text("! ${block.str("reason")}", color = MaterialTheme.colorScheme.error)
        "question" -> {
            val questionId = block.str("question_id")
            val question = state.questions.firstOrNull { it.str("id") == questionId }
            val questionText = question?.str("text").orEmpty().ifBlank { questionId }
            var freeText by remember(questionId) { mutableStateOf("") }
            Text("？$questionText", style = MaterialTheme.typography.titleSmall)
            if (question != null && question.str("state") == "pending") {
                question.arr("options").forEachIndexed { index, option ->
                    FilterChip(selected = false, onClick = { onQuestion(questionId, index, null) }, label = { Text(option.toString().trim('"')) })
                }
                if (question.boolean("allow_free_text")) {
                    OutlinedTextField(freeText, { freeText = it }, Modifier.fillMaxWidth(), label = { Text(stringResource(Res.string.feature_answer)) })
                    Button(onClick = { onQuestion(questionId, null, freeText) }, enabled = freeText.isNotBlank()) { Text(stringResource(Res.string.feature_submit)) }
                }
            }
        }
        "project_card" -> {
            val projectId = block.str("project_id")
            if (projectId.isNotBlank()) Button(onClick = { onOpenProject(projectId) }) { Text(stringResource(Res.string.feature_detail)) }
        }
        "review_card" -> {
            val projectId = block.str("project_id")
            Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                Text(stringResource(Res.string.feature_review_state, ""), style = MaterialTheme.typography.bodyMedium)
                StatusLabel(block.str("state"))
            }
            if (projectId.isNotBlank() && block.str("state") == "pending") Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Button(onClick = { onProjectAction(projectId, "confirm_done") }) { Text(stringResource(Res.string.feature_confirm_done)) }
                Button(onClick = { onProjectAction(projectId, "request_changes") }) { Text(stringResource(Res.string.feature_request_changes)) }
            }
            block.arr("artifacts").forEach { element ->
                val artifact = element as? JsonObject ?: return@forEach
                val artifactId = artifact.str("artifact_id")
                val pathOrUrl = artifact.str("path_or_url")
                if (artifactId.isNotBlank() && pathOrUrl.isNotBlank()) {
                    Button(onClick = { onOpenArtifact(artifactId, pathOrUrl, projectId.takeIf { it.isNotBlank() }) }) {
                        Text("▤ ${artifact.str("title").ifBlank { pathOrUrl }}")
                    }
                }
            }
        }
        "delegation" -> {
            val assignmentId = block.str("assignment_id")
            val bot = state.bots.firstOrNull { it.str("id") == block.str("bot_id") }
            Text("↪ ${bot?.str("name").orEmpty().ifBlank { block.str("bot_id") }}", style = MaterialTheme.typography.bodyMedium)
            if (assignmentId.isNotBlank()) Button(onClick = { onOpenTrace(assignmentId, null) }) { Text(stringResource(Res.string.feature_detail)) }
        }
        "approval" -> {
            val approvalId = block.str("approval_id")
            val approval = state.approvals.firstOrNull { it.str("id") == approvalId }
            Text(stringResource(Res.string.feature_approval_needed), style = MaterialTheme.typography.titleSmall)
            Text(approval?.str("summary").orEmpty().ifBlank { approvalId })
            MarkdownText(approval?.str("detail").orEmpty(), Modifier.fillMaxWidth())
            if (approval?.str("state") == "pending") Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Button(onClick = { onApproval(approvalId, "allow_once") }) { Text(stringResource(Res.string.feature_allow_once)) }
                Button(onClick = { onApproval(approvalId, "always_allow") }) { Text(stringResource(Res.string.feature_always_allow)) }
                Button(onClick = { onApproval(approvalId, "deny") }) { Text(stringResource(Res.string.feature_deny)) }
            }
        }
        "approval_ref" -> {
            Text(stringResource(Res.string.feature_approval_ref), style = MaterialTheme.typography.bodyMedium)
            val chatId = block.str("chat_id")
            if (chatId.isNotBlank()) Button(onClick = { onOpenChat(chatId) }) { Text(stringResource(Res.string.feature_open_chat)) }
        }
        "takeover_request" -> {
            Text("✋ ${block.str("reason")}", style = MaterialTheme.typography.bodyMedium)
            val botId = block.str("bot_id")
            if (block.str("state") != "done" && botId.isNotBlank()) Button(onClick = { onTakeover(botId) }) { Text(stringResource(Res.string.feature_takeover)) }
            if (botId.isNotBlank()) Button(onClick = { onOpenScreen(botId, null) }) { Text(stringResource(Res.string.feature_open_screen)) }
        }
        "bot_dm_ref" -> {
            Text(stringResource(Res.string.feature_bot_dm, block.str("count")), style = MaterialTheme.typography.bodyMedium)
            block.str("chat_id").takeIf { it.isNotBlank() }?.let { Button(onClick = { onOpenChat(it) }) { Text(stringResource(Res.string.feature_open_chat)) } }
        }
        "system" -> Text(block.str("text"), style = MaterialTheme.typography.bodyMedium)
        "loop_paused" -> {
            val root = block.str("root_message_id")
            Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                Text(stringResource(Res.string.feature_loop_paused_label, block.str("hops")), style = MaterialTheme.typography.bodyMedium)
                StatusLabel(block.str("state"))
            }
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Button(onClick = { onLoopAction(root, "continue") }) { Text(stringResource(Res.string.feature_loop_continue)) }
                Button(onClick = { onLoopAction(root, "end") }) { Text(stringResource(Res.string.feature_loop_end)) }
            }
        }
    }
}

internal fun fileRef(block: JsonObject): JsonObject {
    val file = block.obj("file")
    return if (file.str("path").isNotBlank() || file.str("root").isNotBlank()) file else block
}

internal fun fileKey(file: JsonObject): String = "${file.str("root")}:${file.str("root_id")}:${file.str("path")}"
internal fun fileParams(file: JsonObject): Map<String, String> = mapOf("root" to file.str("root"), "root_id" to file.str("root_id"), "path" to file.str("path"))

@Composable
private fun DeliveryView(delivery: JsonArray, state: MobileState) {
    delivery.mapNotNull { it as? JsonObject }.forEach { item ->
        val botId = item.str("bot_id")
        val botName = state.bots.firstOrNull { it.str("id") == botId }?.str("name").orEmpty().ifBlank { botId }
        val stateText = when (item.str("state")) {
            "queued" -> stringResource(Res.string.feature_delivery_queued)
            "delivered" -> stringResource(Res.string.feature_delivery_delivered)
            "read" -> stringResource(Res.string.feature_delivery_read)
            else -> stringResource(Res.string.feature_delivery_unknown)
        }
        Text("$botName · $stateText", style = MaterialTheme.typography.labelSmall)
    }
}

private fun JsonObject.longValue(key: String): Long? = this[key]?.toString()?.trim('"')?.toLongOrNull()
