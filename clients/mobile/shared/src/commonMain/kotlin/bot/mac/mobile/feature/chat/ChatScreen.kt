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
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
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
    onOpenGroupInfo: (projectId: String) -> Unit = {},
    onOpenFollowup: (chatId: String) -> Unit = {},
    onQuoteAnnouncement: (String) -> Unit = {},
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
    var highlightedMessageId by remember(chatId) { mutableStateOf<String?>(null) }
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
    val quoteOriginalUnloaded = stringResource(Res.string.feature_quote_original_unloaded)
    val groupInfoLabel = stringResource(Res.string.feature_group_info)
    val historyLabel = stringResource(Res.string.feature_chat_history)
    val cancelQuoteLabel = stringResource(Res.string.feature_cancel_quote)
    val attachmentLabel = stringResource(Res.string.feature_attachment)
    val mentionLabel = stringResource(Res.string.feature_mention_title)
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
                    IconButton(onClick = { onOpenGroupInfo(projectId) }, modifier = Modifier.semantics { contentDescription = groupInfoLabel }) { Text("ⓘ") }
                    IconButton(onClick = { onOpenProject(projectId) }) { Text("›") }
                }
                if (chat?.str("kind") in setOf("main", "direct")) {
                    TextButton(onClick = { onOpenFollowup(chatId) }) {
                        Text(stringResource(Res.string.feature_followup))
                    }
                    TextButton(onClick = { onOpenTrace(null, chatId) }) {
                        Text(stringResource(Res.string.feature_trace))
                    }
                }
                IconButton(onClick = { onOpenHistory(chatId) }, modifier = Modifier.semantics { contentDescription = historyLabel }) { Text("↺") }
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
                    repository = repository,
                    onQuoteAnnouncement = onQuoteAnnouncement,
                    onJumpToMessage = { quotedId ->
                        val index = messages.indexOfFirst { it.str("id") == quotedId }
                        if (index >= 0) {
                            scope.launch {
                                listState.animateScrollToItem(index + if (historyHasMore) 1 else 0)
                                highlightedMessageId = quotedId
                                kotlinx.coroutines.delay(600)
                                if (highlightedMessageId == quotedId) highlightedMessageId = null
                            }
                        } else error = quoteOriginalUnloaded
                    },
                    highlighted = highlightedMessageId == message.str("id"),
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
                    onReleaseTakeover = { botId -> scope.launch { runCatching { repository.call("takeover.release", buildJsonObject { put("bot_id", botId) }) }.onFailure { error = it.message ?: sendError } } },
                    onOpenArtifact = onOpenArtifact,
                    artifactProjectId = artifactProjectId ?: chat?.str("project_id")?.takeIf { it.isNotBlank() },
                state = state,
                )
            }
        }
        replyTo?.let { id ->
            Surface(Modifier.fillMaxWidth(), color = MaterialTheme.colorScheme.surfaceContainerHigh) {
                Row(Modifier.padding(horizontal = 12.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                    val quoted = messages.firstOrNull { it.str("id") == id }
                    val sender = quoted?.obj("sender")?.str("bot_id")?.let { botId -> state.bots.firstOrNull { it.str("id") == botId }?.str("name") }
                        ?.takeIf(String::isNotBlank) ?: stringResource(Res.string.feature_you)
                    Spacer(Modifier.width(2.dp).heightIn(min = 36.dp).background(MaterialTheme.colorScheme.primary))
                    Column(Modifier.weight(1f).padding(horizontal = 8.dp)) {
                        Text(
                            stringResource(if (quoteMode == "reference") Res.string.feature_quote_to else Res.string.feature_reply_to, sender),
                            style = MaterialTheme.typography.labelMedium,
                            color = MaterialTheme.colorScheme.primary,
                            maxLines = 1,
                        )
                        Text(
                            quoted?.str("fallback_text")?.takeIf { it.isNotBlank() }?.take(96)
                                ?: stringResource(Res.string.feature_reply_message, id.take(8)),
                            style = MaterialTheme.typography.bodySmall,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                    IconButton(onClick = { replyTo = null; quoteMode = "reply" }, modifier = Modifier.semantics { contentDescription = cancelQuoteLabel }) { Text("×") }
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
            }, modifier = Modifier.semantics { contentDescription = attachmentLabel }) { Text("＋") }
            OutlinedTextField(
                value = draft,
                onValueChange = { draft = it; if (it.endsWith("@")) showMentions = true },
                modifier = Modifier.weight(1f),
                placeholder = { Text(stringResource(Res.string.feature_send)) },
                maxLines = 4,
            )
            if (groupChat || mentionCandidates.isNotEmpty()) IconButton(onClick = { showMentions = true }, modifier = Modifier.semantics { contentDescription = mentionLabel }) { Text("@", color = LocalMobilePalette.current.accent) }
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
    repository: MobileRepository,
    onQuoteAnnouncement: (String) -> Unit,
    state: MobileState,
    onJumpToMessage: (String) -> Unit,
    highlighted: Boolean,
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
    onReleaseTakeover: (String) -> Unit,
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
            border = if (highlighted) androidx.compose.foundation.BorderStroke(2.dp, MaterialTheme.colorScheme.primary) else null,
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
                    Surface(
                        color = MaterialTheme.colorScheme.surface.copy(alpha = .55f),
                        shape = RoundedCornerShape(6.dp),
                        onClick = { onJumpToMessage(quote.str("message_id")) },
                    ) {
                        Row(Modifier.fillMaxWidth().padding(end = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                            Spacer(Modifier.width(2.dp).heightIn(min = 34.dp).background(MaterialTheme.colorScheme.primary))
                            Column(Modifier.weight(1f).padding(horizontal = 8.dp, vertical = 5.dp)) {
                            val quoteSenderId = quote.obj("sender").str("bot_id")
                            val quoteSender = state.bots.firstOrNull { it.str("id") == quoteSenderId }?.str("name")
                                ?.takeIf { it.isNotBlank() } ?: stringResource(Res.string.feature_you)
                            Text(quoteSender, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.primary)
                            Text(quote.str("excerpt"), style = MaterialTheme.typography.bodySmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            }
                        }
                    }
                }
                if (message.boolean("streaming")) Text(stringResource(Res.string.feature_typing), style = MaterialTheme.typography.labelMedium)
                val fallback = message.str("fallback_text")
                val unknown = blocks.any { element ->
                    val block = element as? JsonObject ?: return@any true
                    block.str("type") !in KnownBlockTypes
                }
                if (blocks.isEmpty() && fallback.isNotBlank()) Text(fallback, style = MaterialTheme.typography.bodyLarge)
                if (unknown) Surface(Modifier.fillMaxWidth(), color = MaterialTheme.colorScheme.surface, shape = RoundedCornerShape(8.dp), border = androidx.compose.foundation.BorderStroke(1.dp, palette.borderDefault)) {
                    Text("⚠ ${fallback.ifBlank { stringResource(Res.string.feature_unknown_block) }} · ${stringResource(Res.string.feature_update_client)}", Modifier.padding(10.dp), style = MaterialTheme.typography.bodySmall)
                }
                val questionBlocks = blocks.mapNotNull { it as? JsonObject }.filter { it.str("type") == "question" }
                val questionIds = questionBlocks.map { it.str("question_id") }
                var selectedQuestion by remember(message.str("id"), questionIds) {
                    mutableStateOf(questionBlocks.indexOfFirst { block ->
                        state.questions.firstOrNull { it.str("id") == block.str("question_id") }?.str("state") == "pending"
                    }.coerceAtLeast(0))
                }
                LaunchedEffect(questionIds, state.questions.map { it.str("id") to it.str("state") }) {
                    if (questionBlocks.isNotEmpty() && state.questions.firstOrNull { it.str("id") == questionIds.getOrNull(selectedQuestion) }?.str("state") == "answered") {
                        val nextPending = questionBlocks.indexOfFirst { block ->
                            state.questions.firstOrNull { it.str("id") == block.str("question_id") }?.str("state") == "pending"
                        }
                        if (nextPending >= 0) selectedQuestion = nextPending
                    }
                }
                if (questionBlocks.size > 1) {
                    val answered = questionBlocks.count { block -> state.questions.firstOrNull { it.str("id") == block.str("question_id") }?.str("state") == "answered" }
                    val swipeThreshold = with(androidx.compose.ui.platform.LocalDensity.current) { 48.dp.toPx() }
                    Box(
                        Modifier.fillMaxWidth().pointerInput(questionBlocks.size, swipeThreshold) {
                            var drag = 0f
                            detectHorizontalDragGestures(
                                onHorizontalDrag = { change, amount -> drag += amount; change.consume() },
                                onDragEnd = {
                                    when {
                                        drag <= -swipeThreshold -> selectedQuestion = (selectedQuestion + 1).coerceAtMost(questionBlocks.lastIndex)
                                        drag >= swipeThreshold -> selectedQuestion = (selectedQuestion - 1).coerceAtLeast(0)
                                    }
                                    drag = 0f
                                },
                                onDragCancel = { drag = 0f },
                            )
                        },
                    ) {
                        val pendingAfter = (selectedQuestion + 1 until questionBlocks.size).count { index ->
                            state.questions.firstOrNull { it.str("id") == questionIds[index] }?.str("state") == "pending"
                        }.coerceAtMost(2)
                        repeat(pendingAfter) { layer ->
                            Surface(
                                Modifier.fillMaxWidth().padding(horizontal = ((layer + 1) * 8).dp).offset(y = ((layer + 1) * 8).dp).height(148.dp),
                                shape = RoundedCornerShape(12.dp),
                                color = if (layer == 0) palette.panel else palette.panel.copy(alpha = .78f),
                                border = androidx.compose.foundation.BorderStroke(1.dp, palette.borderDefault),
                            ) { }
                        }
                        Column(Modifier.fillMaxWidth().background(palette.panel, RoundedCornerShape(12.dp)).padding(10.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Text(
                            if (answered == questionBlocks.size) stringResource(Res.string.feature_question_stack_answered, questionBlocks.size)
                            else stringResource(Res.string.feature_question_stack_title, questionBlocks.size, selectedQuestion + 1, answered),
                            style = MaterialTheme.typography.labelMedium,
                        )
                        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                            TextButton(enabled = selectedQuestion > 0, onClick = { selectedQuestion-- }) { Text("‹") }
                            Row(Modifier.weight(1f).horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.Center, verticalAlignment = Alignment.CenterVertically) {
                                questionBlocks.forEachIndexed { index, block ->
                                    val status = state.questions.firstOrNull { it.str("id") == block.str("question_id") }?.str("state")
                                    val tabLabel = stringResource(Res.string.feature_question_tab, index + 1)
                                    TextButton(onClick = { selectedQuestion = index }, modifier = Modifier.semantics {
                                        contentDescription = tabLabel
                                    }) {
                                        Text(if (answered == questionBlocks.size) "✓ $tabLabel" else if (status == "answered") "✓ ${index + 1}" else (index + 1).toString())
                                    }
                                }
                            }
                            TextButton(enabled = selectedQuestion < questionBlocks.lastIndex, onClick = { selectedQuestion++ }) { Text("›") }
                        }
                        questionBlocks.getOrNull(selectedQuestion)?.let { block ->
                            BlockView(block, state, onApproval, onQuestion, onPreviewFile, onExportFile, filePreviews, imagePreviews,
                            onOpenTrace, onOpenProject, onProjectAction, onOpenScreen, onOpenChat, onLoopAction, onTakeover, onReleaseTakeover, onOpenArtifact, artifactProjectId, repository)
                        }
                    }
                    }
                }
                var questionStackRendered = false
                blocks.forEach { element ->
                    (element as? JsonObject)?.let { block ->
                        if (block.str("type") == "question" && questionBlocks.size > 1) {
                            if (!questionStackRendered) questionStackRendered = true
                        } else {
                            BlockView(block, state, onApproval, onQuestion, onPreviewFile, onExportFile, filePreviews, imagePreviews,
                                onOpenTrace, onOpenProject, onProjectAction, onOpenScreen, onOpenChat, onLoopAction, onTakeover, onReleaseTakeover, onOpenArtifact, artifactProjectId, repository)
                        }
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
                if (assignmentId.isNotBlank()) TextButton(onClick = { onOpenTrace(assignmentId, null); showMessageActions = false }) {
                    Text(stringResource(Res.string.feature_message_view_work))
                }
                val projectId = artifactProjectId.orEmpty()
                val quoteBody = blocks.mapNotNull { entry -> (entry as? JsonObject)?.takeIf { it.str("type") == "text" }?.str("markdown")?.takeIf(String::isNotBlank) }
                    .joinToString("\n\n").ifBlank { message.str("fallback_text") }
                if (projectId.isNotBlank() && quoteBody.isNotBlank()) TextButton(onClick = {
                    val quoted = quoteBody.lines().joinToString("\n") { "> $it" }
                    onQuoteAnnouncement("**$senderLabel**\n$quoted")
                    showMessageActions = false
                }) { Text(stringResource(Res.string.feature_message_quote_announcement)) }
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
    onReleaseTakeover: (String) -> Unit,
    onOpenArtifact: (String, String, String?) -> Unit,
    artifactProjectId: String?,
    repository: MobileRepository,
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
                val targetChat = state.chats.firstOrNull { it.str("bot_id") == botId && it.str("kind") in listOf("direct", "bot_dm") }?.str("id")
                if (targetChat != null) TextButton(onClick = { onOpenChat(targetChat) }) {
                    Text(stringResource(Res.string.feature_completion_next, botName), style = MaterialTheme.typography.titleSmall)
                } else Text(stringResource(Res.string.feature_completion_next, botName), style = MaterialTheme.typography.titleSmall)
                MarkdownText(handoff.str("instruction"), Modifier.fillMaxWidth())
            }
            if (block.boolean("notify_main")) Text(stringResource(Res.string.feature_completion_notify_main), style = MaterialTheme.typography.labelMedium)
            block.arr("artifacts").forEach { element ->
                val artifact = element as? JsonObject ?: return@forEach
                val url = artifact.str("path_or_url")
                val author = state.bots.firstOrNull { it.str("id") == artifact.str("bot_id") }?.str("name").orEmpty()
                    .ifBlank { artifact.str("bot_id") }
                Column {
                    Text(stringResource(Res.string.feature_artifact_author, author), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    Button(onClick = { onOpenArtifact(artifact.str("artifact_id"), url, artifactProjectId) }) { Text("▤ ${artifact.str("title").ifBlank { url }}") }
                }
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
            Row(Modifier.padding(12.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Spacer(Modifier.width(2.dp).heightIn(min = 56.dp).background(MaterialTheme.colorScheme.primary))
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Text(block.str("site"), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.primary)
                    Text(block.str("title"), style = MaterialTheme.typography.titleSmall)
                    block.str("description").takeIf { it.isNotBlank() }?.let { Text(it, style = MaterialTheme.typography.bodySmall, maxLines = 3) }
                    Text(block.str("url"), style = MaterialTheme.typography.bodySmall, maxLines = 1)
                }
                val image = fileRef(block.obj("image"))
                imagePreviews[fileKey(image)]?.let { Image(it, contentDescription = null, modifier = Modifier.size(56.dp), contentScale = ContentScale.Crop) }
            }
        }
        "question" -> {
            val questionId = block.str("question_id")
            val question = state.questions.firstOrNull { it.str("id") == questionId }
            val questionText = question?.str("text").orEmpty().ifBlank { questionId }
            val questionBot = state.bots.firstOrNull { it.str("id") == question?.str("bot_id") }
                ?.str("name")?.takeIf(String::isNotBlank) ?: stringResource(Res.string.feature_bot)
            var freeText by remember(questionId) { mutableStateOf("") }
            var showFreeText by remember(questionId) { mutableStateOf(false) }
            var selectedOptions by remember(questionId) { mutableStateOf(emptySet<Int>()) }
            val freeTextFocus = remember(questionId) { FocusRequester() }
            LaunchedEffect(showFreeText) { if (showFreeText) runCatching { freeTextFocus.requestFocus() } }
            Text(
                stringResource(if (question?.boolean("multi") == true) Res.string.feature_question_kind_multi else Res.string.feature_question_kind_single, questionBot),
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.primary,
            )
            Text(questionText, style = MaterialTheme.typography.titleSmall)
            when {
                question?.str("state") == "expired" -> {
                    question.arr("options").forEach { option ->
                        Text(option.toString().trim('"'), color = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = .4f))
                    }
                    Text(stringResource(Res.string.feature_decision_expired), color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                question?.str("state") == "answered" -> {
                    val answer = question.obj("answer")
                    val indexes = answer.arr("option_indexes").mapNotNull { it.toString().trim('"').toIntOrNull() }
                    val single = answer["option_index"]?.toString()?.trim('"')?.toIntOrNull()?.let(::listOf).orEmpty()
                    val selected = if (indexes.isNotEmpty()) indexes else single
                    val selectedLabels = selected.mapNotNull { question.arr("options").getOrNull(it)?.toString()?.trim('"') }.joinToString("、")
                    Text(
                        if (selected.isNotEmpty()) stringResource(Res.string.feature_question_answered_option, selectedLabels)
                        else stringResource(Res.string.feature_question_answered_text),
                        color = MaterialTheme.colorScheme.primary,
                        style = MaterialTheme.typography.labelMedium,
                    )
                    question.arr("options").forEachIndexed { index, option ->
                        Text("${if (index in selected) "✓ " else ""}${option.toString().trim('"')}", color = if (index in selected) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    answer.str("text").takeIf { it.isNotBlank() }?.let { text ->
                        Surface(color = MaterialTheme.colorScheme.surfaceContainerHigh, shape = RoundedCornerShape(8.dp)) {
                            Row(Modifier.fillMaxWidth().padding(8.dp), verticalAlignment = Alignment.CenterVertically) {
                                Spacer(Modifier.width(2.dp).heightIn(min = 24.dp).background(MaterialTheme.colorScheme.primary))
                                Column(Modifier.padding(start = 8.dp)) {
                                    Text(stringResource(Res.string.feature_question_your_reply), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.primary)
                                    Text(text, style = MaterialTheme.typography.bodyMedium)
                                }
                            }
                        }
                    }
                }
                question?.str("state") == "pending" -> {
                    question.arr("options").forEachIndexed { index, option ->
                        FilterChip(
                            selected = index in selectedOptions,
                            modifier = Modifier.heightIn(min = 44.dp),
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
                        if (!showFreeText) {
                            TextButton(onClick = { showFreeText = true }, modifier = Modifier.heightIn(min = 44.dp)) {
                                Text(stringResource(Res.string.feature_question_other))
                            }
                        } else {
                            OutlinedTextField(
                                freeText,
                                { freeText = it },
                                Modifier.fillMaxWidth().focusRequester(freeTextFocus),
                                label = { Text(stringResource(Res.string.feature_answer)) },
                            )
                            Button(onClick = { onQuestion(questionId, null, null, freeText) }, enabled = freeText.isNotBlank()) { Text(stringResource(Res.string.feature_submit)) }
                        }
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
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Surface(Modifier.size(40.dp), shape = RoundedCornerShape(12.dp), color = MaterialTheme.colorScheme.secondaryContainer) { Box(contentAlignment = Alignment.Center) { Text("◉", color = MaterialTheme.colorScheme.primary) } }
                        Text(project?.str("name").orEmpty().ifBlank { projectId }, Modifier.weight(1f), style = MaterialTheme.typography.titleSmall)
                        project?.str("status")?.takeIf { it.isNotBlank() }?.let { StatusLabel(it) }
                    }
                    Text(stringResource(Res.string.feature_project_card_members, project?.arr("members")?.size ?: 0), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    block.str("note").takeIf { it.isNotBlank() }?.let { Text(it, style = MaterialTheme.typography.bodySmall, maxLines = 2) }
                    Text(stringResource(Res.string.feature_open_group), style = MaterialTheme.typography.labelMedium)
                }
            }
        }
        "review_card" -> {
            val projectId = block.str("project_id")
            val reviewState = block.str("state")
            Text(
                stringResource(Res.string.feature_review_project, reviewProjectLabel(block, state.projects)),
                style = MaterialTheme.typography.titleSmall,
            )
            Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                Text(stringResource(Res.string.feature_review_state, ""), style = MaterialTheme.typography.bodyMedium)
                StatusLabel(block.str("state"))
            }
            when (reviewState) {
                "confirmed" -> Text(stringResource(Res.string.feature_review_confirmed), color = MaterialTheme.colorScheme.tertiary, style = MaterialTheme.typography.labelMedium)
                "changes_requested" -> Text(stringResource(Res.string.feature_review_changes_requested), color = MaterialTheme.colorScheme.secondary, style = MaterialTheme.typography.labelMedium)
                "pending" -> if (projectId.isNotBlank()) Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    Button(onClick = { onProjectAction(projectId, "confirm_done") }, modifier = Modifier.fillMaxWidth().heightIn(min = 44.dp)) {
                        Text(stringResource(Res.string.feature_confirm_done))
                    }
                    Button(onClick = { onProjectAction(projectId, "request_changes") }, modifier = Modifier.fillMaxWidth().heightIn(min = 44.dp)) {
                        Text(stringResource(Res.string.feature_request_changes))
                    }
                }
            }
            block.arr("artifacts").forEach { element ->
                val artifact = element as? JsonObject ?: return@forEach
                val artifactId = artifact.str("artifact_id")
                val pathOrUrl = artifact.str("path_or_url")
                if (artifactId.isNotBlank() && pathOrUrl.isNotBlank()) {
                    Surface(
                        modifier = Modifier.fillMaxWidth(),
                        color = MaterialTheme.colorScheme.surfaceContainerHigh,
                        shape = RoundedCornerShape(10.dp),
                        onClick = { onOpenArtifact(artifactId, pathOrUrl, projectId.takeIf { it.isNotBlank() }) },
                    ) {
                        Column(Modifier.padding(10.dp)) {
                            Text(artifact.str("title").ifBlank { pathOrUrl }, style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            val authorId = artifact.str("bot_id")
                            val author = state.bots.firstOrNull { it.str("id") == authorId }?.str("name").orEmpty().ifBlank { authorId }
                            if (author.isNotBlank()) Text(stringResource(Res.string.feature_artifact_author, author), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                            Text(pathOrUrl, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            Text(stringResource(Res.string.feature_preview_file), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.primary)
                        }
                    }
                }
            }
            if (projectId.isNotBlank()) TextButton(onClick = { onOpenProject(projectId) }, modifier = Modifier.fillMaxWidth().heightIn(min = 44.dp)) {
                Text(stringResource(Res.string.feature_review_enter_group))
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
            } else if (approval != null) StatusLabel(approval.str("state"))
        }
        "approval_ref" -> {
            Text(stringResource(Res.string.feature_approval_ref), style = MaterialTheme.typography.bodyMedium)
            val chatId = block.str("chat_id")
            if (chatId.isNotBlank()) Button(onClick = { onOpenChat(chatId) }) { Text(stringResource(Res.string.feature_open_chat)) }
        }
        "takeover_request" -> {
            val botId = block.str("bot_id")
            if (block.str("reason").isNotBlank()) Text(block.str("reason"), style = MaterialTheme.typography.bodyMedium)
            when (block.str("state")) {
                "pending" -> {
                    Text(stringResource(Res.string.feature_takeover_pending), style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.tertiary)
                    if (botId.isNotBlank()) Button(onClick = { onTakeover(botId) }, modifier = Modifier.fillMaxWidth().heightIn(min = 44.dp)) {
                        Text(stringResource(Res.string.feature_takeover))
                    }
                }
                "active" -> {
                    Text(stringResource(Res.string.feature_takeover_active), style = MaterialTheme.typography.labelMedium)
                    if (botId.isNotBlank()) {
                        Button(onClick = { onOpenScreen(botId, null) }, modifier = Modifier.fillMaxWidth().heightIn(min = 44.dp)) {
                            Text(stringResource(Res.string.feature_open_screen))
                        }
                        TextButton(onClick = { onReleaseTakeover(botId) }, modifier = Modifier.fillMaxWidth().heightIn(min = 44.dp)) {
                            Text(stringResource(Res.string.feature_return_control))
                        }
                    }
                }
                "done" -> Text(stringResource(Res.string.feature_takeover_done), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                else -> StatusLabel(block.str("state"))
            }
        }
        "bot_dm_ref" -> {
            val chatId = block.str("chat_id")
            val count = block.longValue("count")?.toInt() ?: 0
            var expanded by remember(chatId) { mutableStateOf(false) }
            var loading by remember(chatId) { mutableStateOf(false) }
            var loadError by remember(chatId) { mutableStateOf(false) }
            var excerptMessages by remember(chatId) { mutableStateOf(emptyList<JsonObject>()) }
            val participantIds = state.chats.firstOrNull { it.str("id") == chatId }?.arr("member_bot_ids").orEmpty().map { it.toString().trim('"') }
            val participants = participantIds.mapNotNull { id -> state.bots.firstOrNull { it.str("id") == id } }.take(2)
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("✉", color = MaterialTheme.colorScheme.primary)
                if (participants.size == 2) {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                        participants.forEach { bot ->
                            Surface(Modifier.size(24.dp), shape = RoundedCornerShape(50), color = MaterialTheme.colorScheme.secondaryContainer) {
                                Box(contentAlignment = Alignment.Center) { Text(bot.str("name").take(1), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSecondaryContainer) }
                            }
                        }
                    }
                    Text(stringResource(Res.string.feature_bot_dm_participants, participants[0].str("name"), participants[1].str("name"), count), Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium)
                } else Text(stringResource(Res.string.feature_bot_dm, count.toString()), Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium)
                TextButton(
                    enabled = chatId.isNotBlank() && !loading,
                    onClick = {
                        expanded = !expanded
                        if (expanded && excerptMessages.isEmpty() && !loadError) {
                            loading = true
                            blockScope.launch {
                                runCatching { repository.call("chat.history", buildJsonObject { put("chat_id", chatId); put("limit", count.coerceIn(1, 20)) }) }
                                    .onSuccess { result -> excerptMessages = result.objects("messages").takeLast(count.coerceIn(1, 20)) }
                                    .onFailure { loadError = true }
                                loading = false
                            }
                        }
                    },
                ) { Text(stringResource(if (expanded) Res.string.feature_dm_collapse else Res.string.feature_dm_expand)) }
            }
            if (expanded) {
                when {
                    loading -> Text(stringResource(Res.string.feature_dm_loading), style = MaterialTheme.typography.bodySmall)
                    loadError -> Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(stringResource(Res.string.feature_dm_load_error), Modifier.weight(1f), color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
                        TextButton(onClick = {
                            loadError = false
                            loading = true
                            blockScope.launch {
                                runCatching { repository.call("chat.history", buildJsonObject { put("chat_id", chatId); put("limit", count.coerceIn(1, 20)) }) }
                                    .onSuccess { result -> excerptMessages = result.objects("messages").takeLast(count.coerceIn(1, 20)) }
                                    .onFailure { loadError = true }
                                loading = false
                            }
                        }) { Text(stringResource(Res.string.feature_dm_retry)) }
                    }
                    excerptMessages.isEmpty() -> Text(stringResource(Res.string.feature_dm_empty), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    else -> Column(Modifier.fillMaxWidth().padding(start = 24.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        excerptMessages.forEach { dmMessage ->
                            Surface(color = MaterialTheme.colorScheme.surfaceContainerHigh, shape = RoundedCornerShape(8.dp)) {
                                Column(Modifier.fillMaxWidth().padding(8.dp)) {
                                    val senderId = dmMessage.obj("sender").str("bot_id")
                                    val senderName = state.bots.firstOrNull { it.str("id") == senderId }?.str("name").orEmpty().ifBlank { senderId.ifBlank { stringResource(Res.string.feature_bot) } }
                                    Text(senderName, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.primary)
                                    dmMessage.arr("blocks").forEach { entry ->
                                        val dmBlock = entry as? JsonObject ?: return@forEach
                                        when (dmBlock.str("type")) {
                                            "text" -> MarkdownText(dmBlock.str("markdown"), Modifier.fillMaxWidth())
                                            "system" -> Text(dmBlock.str("text"), style = MaterialTheme.typography.bodySmall)
                                            else -> dmMessage.str("fallback_text").takeIf { it.isNotBlank() }?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        "system" -> if (block.str("code") == "announcement_updated" && !artifactProjectId.isNullOrBlank()) {
            TextButton(onClick = { onOpenProject(artifactProjectId) }) { Text(block.str("text"), style = MaterialTheme.typography.bodyMedium) }
        } else Text(block.str("text"), style = MaterialTheme.typography.bodyMedium)
        "loop_paused" -> {
            val root = block.str("root_message_id")
            Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                Text(stringResource(Res.string.feature_loop_paused_label, block.str("hops")), style = MaterialTheme.typography.bodyMedium)
                StatusLabel(block.str("state"))
            }
            if (block.str("state") == "paused") Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
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
