package bot.mac.mobile.feature.chat

import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
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
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.AnnotatedString
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
import bot.mac.mobile.core.platform.openExternalUrl
import bot.mac.mobile.core.design.LocalMobilePalette
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
import kotlinx.coroutines.launch

@OptIn(ExperimentalMaterial3Api::class)
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
    var quoteMode by remember(chatId) { mutableStateOf("reply") }
    var mentionBots by remember(chatId) { mutableStateOf(emptySet<String>()) }
    var showMentions by remember(chatId) { mutableStateOf(false) }
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
    var scrollState by remember(chatId) { mutableStateOf(ChatScrollState()) }
    var historyLoaded by remember(chatId) { mutableStateOf(false) }
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
        historyLoaded = false
        scrollState = ChatScrollState()
        runCatching {
            repository.call("chat.history", buildJsonObject { put("chat_id", chatId); put("limit", 100) })

        }.onSuccess { result ->
            historyHasMore = result.boolean("has_more")
            historyBeforeSeq = result.objects("messages").minOfOrNull { it.longValue("seq") ?: Long.MAX_VALUE }
                ?.takeIf { it != Long.MAX_VALUE }
            historyLoaded = true
        }.onFailure { error = it.message ?: loadingError }
            .also { if (it.isFailure) historyLoaded = true }
    }
    LaunchedEffect(draft.startsWith("/")) {
        if (draft.startsWith("/")) runCatching { repository.call("skill.list", buildJsonObject {}) }
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

    LaunchedEffect(chatId, historyHasMore, scrollState.initialPositioned) {
        snapshotFlow {
            val lastVisible = listState.layoutInfo.visibleItemsInfo.lastOrNull()?.index
            val lastIndex = listState.layoutInfo.totalItemsCount - 1
            lastVisible == null || lastIndex < 0 || lastVisible >= lastIndex - 1
        }.collect { scrollState = scrollState.onViewportChanged(it) }
    }

    LaunchedEffect(chatId, historyLoaded, messages.size, historyHasMore) {
        if (!historyLoaded || scrollState.initialPositioned) return@LaunchedEffect
        val lastIndex = latestChatItemIndex(messages.size, historyHasMore) ?: return@LaunchedEffect
        withFrameNanos { }
        listState.scrollToItem(lastIndex)
        scrollState = scrollState.afterInitialPosition()
    }

    LaunchedEffect(messages.size, messages.lastOrNull()?.toString(), historyHasMore, scrollState) {
        if (!scrollState.shouldScrollToLatest(messages.size)) return@LaunchedEffect
        val lastIndex = latestChatItemIndex(messages.size, historyHasMore) ?: return@LaunchedEffect
        withFrameNanos { }
        listState.scrollToItem(lastIndex)
    }

    LaunchedEffect(chatId, chat?.longValue("last_seq")) {
        val seq=chat?.longValue("last_seq") ?: return@LaunchedEffect
        runCatching { repository.call("chat.mark_read",buildJsonObject { put("chat_id",chatId);put("seq",seq) }) }
    }
    Column(Modifier.fillMaxSize().background(LocalMobilePalette.current.chat)) {
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
                if (chat?.str("kind") in setOf("main", "direct")) {
                    TextButton(onClick = { onOpenTrace(null, chatId) }) {
                        Text(stringResource(Res.string.feature_trace))
                    }
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
                    onQuote = { id, mode -> quoteMode = mode; replyTo = id },
                    onReact = { id, emoji -> scope.launch { runCatching { repository.call("chat.react", buildJsonObject { put("message_id", id); put("emoji", emoji); put("on", true) }) }.onFailure { error = it.message ?: sendError } } },
                    onOpenTrace = onOpenTrace,
                    onApproval = { id, decision -> scope.launch { runCatching { repository.call("approval.decide", buildJsonObject { put("approval_id", id); put("decision", decision) }) }.onFailure { error = it.message ?: sendError } } },
                    onQuestion = { id, index, indexes, text -> scope.launch { runCatching { repository.call("question.answer", buildJsonObject {
                        put("question_id", id)
                        index?.let { put("option_index", it) }
                        indexes?.let { values -> put("option_indexes", buildJsonArray { values.forEach { add(JsonPrimitive(it)) } }) }
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
        replyTo?.let { id ->
            Surface(Modifier.fillMaxWidth(), color = MaterialTheme.colorScheme.secondaryContainer) {
                Row(Modifier.padding(horizontal = 16.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                    val quoted = messages.firstOrNull { it.str("id") == id }
                    Text(
                        quoted?.str("fallback_text")?.takeIf { it.isNotBlank() }?.let {
                            "${stringResource(if (quoteMode == "reference") Res.string.feature_quote_reference else Res.string.feature_reply)} · ${it.take(96)}"
                        } ?: stringResource(Res.string.feature_reply_message, id.take(8)),
                        Modifier.weight(1f), style = MaterialTheme.typography.labelMedium, maxLines = 2,
                    )
                    IconButton(onClick = { replyTo = null; quoteMode = "reply" }) { Text("×") }
                }
            }
        }
        error?.let { Text(it, Modifier.padding(horizontal = 12.dp), color = MaterialTheme.colorScheme.error) }
        attachmentNames.forEach { name -> Text(stringResource(Res.string.feature_attachment_added, name), Modifier.padding(horizontal = 12.dp), style = MaterialTheme.typography.labelSmall) }
        val slashQuery = draft.takeIf { it.startsWith("/") }?.substringAfter('/').orEmpty().substringBefore(' ')
        if (draft.startsWith("/") && !draft.substringAfter('/', "").contains(' ')) {
            Surface(Modifier.fillMaxWidth().padding(horizontal = 12.dp), color = LocalMobilePalette.current.sidebar, shape = RoundedCornerShape(10.dp)) {
                Column(Modifier.heightIn(max = 340.dp).verticalScroll(rememberScrollState()).padding(8.dp)) {
                    val matches = state.skills.filter { it.str("name").contains(slashQuery, ignoreCase = true) }
                    matches.forEach { skill ->
                        TextButton(onClick = { draft = "/${skill.str("name")} "; error = null }) {
                            Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.Start) {
                                Text(skill.str("name"), style = MaterialTheme.typography.titleSmall)
                                Text(skill.str("description"), style = MaterialTheme.typography.bodySmall, maxLines = 2)
                            }
                        }
                    }
                    if (matches.isEmpty()) Text(stringResource(Res.string.feature_skill_suggestions_empty), Modifier.padding(12.dp), color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
        }
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
                onValueChange = { draft = it; if (it.endsWith("@")) showMentions = true },
                modifier = Modifier.weight(1f),
                placeholder = { Text(stringResource(Res.string.feature_send)) },
                maxLines = 4,
            )
            if (groupChat || mentionCandidates.isNotEmpty()) IconButton(onClick = { showMentions = true }) { Text("@", color = LocalMobilePalette.current.accent) }
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
                                replyTo?.let { put("quote", it); put("quote_mode", quoteMode) }
                            })
                        }.onSuccess {
                            draft = ""
                            replyTo = null
                            quoteMode = "reply"
                            mentionBots = emptySet()
                            attachments = emptyList()
                            attachmentNames = emptyList()
                            error = null
                        }.onFailure { error = it.message ?: sendError }
                    }
                },
            ) { Text(if (readOnly) stringResource(Res.string.feature_readonly) else stringResource(Res.string.feature_send)) }
        }
        if (showMentions) androidx.compose.material3.ModalBottomSheet(onDismissRequest = { showMentions = false }) {
            Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(stringResource(Res.string.feature_mention_title), style = MaterialTheme.typography.titleMedium)
                if (groupChat) TextButton(onClick = {
                    mentionBots = mentionBots + "__everyone__"
                    draft = draft.removeSuffix("@") + "@everyone "
                    showMentions = false
                }) { Text(stringResource(Res.string.feature_mention_everyone)) }
                mentionCandidates.forEach { bot ->
                    val id = bot.str("id")
                    TextButton(onClick = {
                        mentionBots = mentionBots + id
                        draft = draft.removeSuffix("@") + "@${bot.str("name")} "
                        showMentions = false
                    }) {
                        Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.Start) {
                            Text(bot.str("name"), style = MaterialTheme.typography.titleSmall)
                            Text(
                                if (bot.obj("status").str("summary") in setOf("working", "queued")) stringResource(Res.string.feature_mention_steer) else stringResource(Res.string.feature_idle),
                                style = MaterialTheme.typography.bodySmall,
                            )
                        }
                    }
                }
            }
        }
    }
}

private data class ScrollAnchor(val id: String, val offset: Int)

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun ChatMessageRow(
    message: JsonObject,
    state: MobileState,
    onReply: (String) -> Unit,
    onQuote: (String, String) -> Unit,
    onReact: (String, String) -> Unit,
    onOpenTrace: (String?, String?) -> Unit,
    onApproval: (String, String) -> Unit,
    onQuestion: (String, Int?, List<Int>?, String?) -> Unit,
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
    val palette = LocalMobilePalette.current
    var showMessageActions by remember(message.str("id")) { mutableStateOf(false) }
    val haptics = LocalHapticFeedback.current
    val clipboard = LocalClipboardManager.current
    val swipeThreshold = with(androidx.compose.ui.platform.LocalDensity.current) { 56.dp.toPx() }
    var didSwipeReply by remember(message.str("id")) { mutableStateOf(false) }
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
            modifier = Modifier
                .pointerInput(message.str("id"), swipeThreshold) {
                    var distance = 0f
                    detectHorizontalDragGestures(
                        onHorizontalDrag = { change, amount ->
                            if (amount > 0) { distance += amount; change.consume() }
                        },
                        onDragEnd = {
                            if (distance >= swipeThreshold && id.isNotBlank()) {
                                didSwipeReply = true
                                haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                                onReply(id)
                            }
                            distance = 0f
                        },
                        onDragCancel = { distance = 0f },
                    )
                },
            shape = RoundedCornerShape(16.dp),
            color = if (isUser) palette.bubbleOut else palette.bubbleIn,
            contentColor = if (isUser) palette.bubbleOutText else palette.bubbleInText,
        ) {
            Column(
                Modifier.fillMaxWidth(0.9f).combinedClickable(
                    onClick = { if (didSwipeReply) didSwipeReply = false },
                    onLongClick = {
                        haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                        showMessageActions = true
                    },
                    onDoubleClick = { if (id.isNotBlank()) onReact(id, "👍") },
                ).padding(horizontal = 12.dp, vertical = 8.dp),
            ) {
                val quote = message.obj("quote")
                if (quote.str("message_id").isNotBlank()) {
                    Surface(color = MaterialTheme.colorScheme.surface.copy(alpha = .55f), shape = RoundedCornerShape(6.dp)) {
                        Column(Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 5.dp)) {
                            val quoteSenderId = quote.obj("sender").str("bot_id")
                            val quoteSender = state.bots.firstOrNull { it.str("id") == quoteSenderId }?.str("name")
                                ?.takeIf { it.isNotBlank() } ?: stringResource(Res.string.feature_you)
                            Text(quoteSender, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.primary)
                            Text(quote.str("excerpt"), style = MaterialTheme.typography.bodySmall, maxLines = 2)
                        }
                    }
                }
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
                DeliveryView(message.arr("delivery"), state)
                Text(
                    message.str("created_at").substringAfter('T').take(5),
                    Modifier.align(Alignment.End), style = MaterialTheme.typography.labelSmall,
                    color = if (isUser) palette.textSecondary else palette.textTertiary,
                )
            }
        }
        if (showMessageActions) androidx.compose.material3.ModalBottomSheet(onDismissRequest = { showMessageActions = false }) {
            Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(stringResource(Res.string.feature_message_actions), style = MaterialTheme.typography.titleMedium)
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceEvenly) {
                    listOf("👍", "❤️", "😂", "🎉", "🤔", "👀").forEach { emoji ->
                        TextButton(onClick = { if (id.isNotBlank()) onReact(id, emoji); showMessageActions = false }) { Text(emoji, style = MaterialTheme.typography.titleLarge) }
                    }
                }
                TextButton(onClick = { if (id.isNotBlank()) onQuote(id, "reply"); showMessageActions = false }) { Text(stringResource(Res.string.feature_reply)) }
                TextButton(onClick = { if (id.isNotBlank()) onQuote(id, "reference"); showMessageActions = false }) { Text(stringResource(Res.string.feature_quote_reference)) }
                TextButton(onClick = { clipboard.setText(AnnotatedString(message.str("fallback_text"))); showMessageActions = false }) { Text(stringResource(Res.string.feature_copy_message)) }
            }
        }
    }
}

private val KnownBlockTypes = setOf(
    "text", "image", "file", "task_card", "completion", "progress", "blocked", "question", "project_card", "review_card",
    "delegation", "approval", "approval_ref", "takeover_request", "bot_dm_ref", "system", "loop_paused", "link_preview",
)

@Composable
private fun BlockView(
    block: JsonObject,
    state: MobileState,
    onApproval: (String, String) -> Unit,
    onQuestion: (String, Int?, List<Int>?, String?) -> Unit,
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
    val blockScope = rememberCoroutineScope()
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
        "link_preview" -> Surface(
            modifier = Modifier.fillMaxWidth(),
            color = MaterialTheme.colorScheme.surfaceContainerHigh,
            shape = RoundedCornerShape(10.dp),
            onClick = { blockScope.launch { openExternalUrl(block.str("url")) } },
        ) {
            Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text(block.str("site"), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.primary)
                Text(block.str("title"), style = MaterialTheme.typography.titleSmall)
                block.str("description").takeIf { it.isNotBlank() }?.let { Text(it, style = MaterialTheme.typography.bodySmall, maxLines = 3) }
                Text(block.str("url"), style = MaterialTheme.typography.bodySmall, maxLines = 1)
            }
        }
        "question" -> {
            val questionId = block.str("question_id")
            val question = state.questions.firstOrNull { it.str("id") == questionId }
            val questionText = question?.str("text").orEmpty().ifBlank { questionId }
            var freeText by remember(questionId) { mutableStateOf("") }
            var selectedOptions by remember(questionId) { mutableStateOf(emptySet<Int>()) }
            Text(stringResource(Res.string.feature_decision_title, questionText), style = MaterialTheme.typography.titleSmall)
            when {
                question?.str("state") == "expired" -> Text(stringResource(Res.string.feature_decision_expired), color = MaterialTheme.colorScheme.onSurfaceVariant)
                question?.str("state") == "answered" -> {
                    val answer = question.obj("answer")
                    val indexes = answer.arr("option_indexes").mapNotNull { it.toString().trim('"').toIntOrNull() }
                    val single = answer["option_index"]?.toString()?.trim('"')?.toIntOrNull()?.let(::listOf).orEmpty()
                    val selected = if (indexes.isNotEmpty()) indexes else single
                    question.arr("options").forEachIndexed { index, option ->
                        Text("${if (index in selected) "✓ " else ""}${option.toString().trim('"')}", color = if (index in selected) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    answer.str("text").takeIf { it.isNotBlank() }?.let { Text(it) }
                }
                question?.str("state") == "pending" -> {
                    question.arr("options").forEachIndexed { index, option ->
                        FilterChip(
                            selected = index in selectedOptions,
                            onClick = {
                                if (question.boolean("multi")) selectedOptions = if (index in selectedOptions) selectedOptions - index else selectedOptions + index
                                else onQuestion(questionId, index, null, null)
                            },
                            label = { Text(option.toString().trim('"')) },
                        )
                    }
                    if (question.boolean("multi")) Button(
                        onClick = { onQuestion(questionId, null, selectedOptions.sorted(), null) },
                        enabled = selectedOptions.isNotEmpty(),
                    ) { Text(stringResource(Res.string.feature_submit)) }
                    if (question.boolean("allow_free_text")) {
                        OutlinedTextField(freeText, { freeText = it }, Modifier.fillMaxWidth(), label = { Text(stringResource(Res.string.feature_answer)) })
                        Button(onClick = { onQuestion(questionId, null, null, freeText) }, enabled = freeText.isNotBlank()) { Text(stringResource(Res.string.feature_submit)) }
                    }
                }
            }
        }
        "project_card" -> {
            val projectId = block.str("project_id")
            val project = state.projects.firstOrNull { it.str("id") == projectId }
            Surface(
                modifier = Modifier.fillMaxWidth(), color = MaterialTheme.colorScheme.surfaceContainerHigh,
                shape = RoundedCornerShape(10.dp), onClick = { if (projectId.isNotBlank()) onOpenProject(projectId) },
            ) {
                Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Text(stringResource(if (block.str("context") == "created") Res.string.feature_project_created else Res.string.feature_project_mentioned), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.primary)
                    Text(project?.str("name").orEmpty().ifBlank { projectId }, style = MaterialTheme.typography.titleSmall)
                    block.str("note").takeIf { it.isNotBlank() }?.let { Text(it, style = MaterialTheme.typography.bodySmall, maxLines = 2) }
                    project?.str("status")?.takeIf { it.isNotBlank() }?.let { StatusLabel(it) }
                    Text(stringResource(Res.string.feature_open_group), style = MaterialTheme.typography.labelMedium)
                }
            }
        }
        "review_card" -> {
            val projectId = block.str("project_id")
            Text(
                stringResource(Res.string.feature_review_project, reviewProjectLabel(block, state.projects)),
                style = MaterialTheme.typography.titleSmall,
            )
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
