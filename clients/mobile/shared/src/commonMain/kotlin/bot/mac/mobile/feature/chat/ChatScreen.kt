package bot.mac.mobile.feature.chat

import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.clickable
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
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
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.key.type
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.isTraversalGroup
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.IntOffset
import kotlin.math.roundToInt
import bot.mac.mobile.core.protocol.arr
import bot.mac.mobile.core.protocol.withClientLocale
import bot.mac.mobile.core.protocol.boolean
import bot.mac.mobile.core.protocol.obj
import bot.mac.mobile.core.protocol.objects
import bot.mac.mobile.core.protocol.long
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.protocol.generated.Approval as ApprovalWireModel
import bot.mac.mobile.core.platform.PickedFile
import bot.mac.mobile.core.platform.FilePickerMode
import bot.mac.mobile.core.platform.exportFile
import bot.mac.mobile.core.platform.platformFilePicker
import bot.mac.mobile.core.platform.platformLocaleTag
import bot.mac.mobile.core.platform.platformScreenImageDecoder
import bot.mac.mobile.core.platform.platformPersistentStore
import bot.mac.mobile.core.platform.openExternalUrl
import bot.mac.mobile.core.design.BotAvatar
import bot.mac.mobile.core.design.FontScaleLimit
import bot.mac.mobile.core.design.LocalMobilePalette
import bot.mac.mobile.core.design.LocalSystemAnimationsEnabled
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.core.state.MobileState
import bot.mac.mobile.core.ui.MarkdownText
import bot.mac.mobile.core.ui.LucideIcon
import bot.mac.mobile.core.ui.LucideIconKind
import bot.mac.mobile.resources.*
import kotlinx.coroutines.launch
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import bot.mac.mobile.core.network.ScreenConnection
import bot.mac.mobile.core.network.RandomIdGenerator
import kotlinx.datetime.TimeZone
import kotlinx.datetime.toLocalDateTime
import kotlin.time.Clock
import kotlin.time.Instant
import kotlin.math.abs
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.jetbrains.compose.resources.stringResource

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ChatScreen(
    repository: MobileRepository,
    chatId: String,
    initialMessageId: String? = null,
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
    onCreateSkill: (String) -> Unit = {},
    artifactProjectId: String? = null,
    showHeader: Boolean = true,
) {
    val state by repository.state.collectAsState()
    val hostId = repository.activeHost.collectAsState().value?.id ?: "active"
    val animationsEnabled = LocalSystemAnimationsEnabled.current
    val messages = state.messages[chatId].orEmpty()
    val lastOwnMessageIndex = messages.indexOfLast { it.obj("sender").str("kind") == "user" }
    val chat = state.chats.firstOrNull { it.str("id") == chatId }
    var draft by remember(hostId, chatId) { mutableStateOf("") }
    var selectedSkill by remember(hostId, chatId) { mutableStateOf<String?>(null) }
    var slashSkills by remember(hostId, chatId) { mutableStateOf(emptyList<JsonObject>()) }
    var replyTo by remember(hostId, chatId) { mutableStateOf<String?>(null) }
    var quoteMode by remember(hostId, chatId) { mutableStateOf("reply") }
    var mentionBots by remember(hostId, chatId) { mutableStateOf(emptySet<String>()) }
    var showMentions by remember(hostId, chatId) { mutableStateOf(false) }
    var attachments by remember(hostId, chatId) { mutableStateOf(emptyList<ChatAttachmentItem>()) }
    var showAttachmentMenu by remember(hostId, chatId) { mutableStateOf(false) }
    var error by remember(hostId, chatId) { mutableStateOf<String?>(null) }
    var filePreviews by remember(hostId, chatId) { mutableStateOf(emptyMap<String, String>()) }
    var imagePreviews by remember(hostId, chatId) { mutableStateOf(emptyMap<String, ImageBitmap>()) }
    var draftLoaded by remember(hostId, chatId) { mutableStateOf(false) }
    var historyHasMore by remember(hostId, chatId) { mutableStateOf(false) }
    var historyBeforeSeq by remember(hostId, chatId) { mutableStateOf<Long?>(null) }
    var loadingOlder by remember(hostId, chatId) { mutableStateOf(false) }
    var pendingAnchor by remember(hostId, chatId) { mutableStateOf<ScrollAnchor?>(null) }
    var highlightedMessageId by remember(hostId, chatId) { mutableStateOf<String?>(null) }
    var exactTimeMessageKey by remember(hostId, chatId) { mutableStateOf<String?>(null) }
    var scrollState by remember(hostId, chatId) { mutableStateOf(ChatScrollState()) }
    var historyLoaded by remember(hostId, chatId) { mutableStateOf(false) }
    val listState = rememberLazyListState()
    val scope = rememberCoroutineScope()
    val composerFocusRequester = remember(hostId, chatId) { FocusRequester() }
    val mentionButtonFocusRequester = remember(hostId, chatId) { FocusRequester() }
    var mentionOpenedByButton by remember(hostId, chatId) { mutableStateOf(false) }
    var initialJumpHandled by remember(hostId, chatId, initialMessageId) { mutableStateOf(initialMessageId.isNullOrBlank()) }
    val readOnly = chat?.str("kind") == "bot_dm"
    val memberIds = chat?.arr("member_bot_ids").orEmpty().map { it.toString().trim('"') }
    val mentionCandidates = state.bots.filter { it.str("id") in memberIds || memberIds.isEmpty() }
    val groupChat = chat?.str("kind") == "project"
    val mainBotChat = chat?.str("kind") == "main"
    val chatBot = chat?.str("bot_id")?.let { botId -> state.bots.firstOrNull { it.str("id") == botId } }
    val showMainChatEmptyState = mainBotChat && messages.isEmpty() && historyLoaded
    val activeBotAssignments = state.assignments.count {
        it.str("bot_id") == chat?.str("bot_id") && it.str("status") in setOf("working", "queued")
    }
    val waitingApproval = state.approvals.firstOrNull { it.str("chat_id") == chatId && it.str("state") == "pending" }
    val waitingQuestion = state.questions.firstOrNull { it.str("chat_id") == chatId && it.str("state") == "pending" }
    val waitingReviewProject = if (chat?.str("kind") == "main") state.projects.firstOrNull {
        it.str("status") in setOf("review", "pending_review")
    } else null
    val workingProjectBots = if (groupChat) {
        state.assignments.asSequence()
            .filter { it.str("project_id") == chat?.str("project_id") && it.str("status") == "working" }
            .map { it.str("bot_id") }
            .filter(String::isNotBlank)
            .distinct()
            .mapNotNull { botId -> state.bots.firstOrNull { it.str("id") == botId } }
            .toList()
    } else emptyList()
    val loadingError = stringResource(Res.string.feature_loading_error)
    val sendError = stringResource(Res.string.feature_send_error)
    val quoteOriginalUnloaded = stringResource(Res.string.feature_quote_original_unloaded)
    val groupInfoLabel = stringResource(Res.string.feature_group_info)
    val historyLabel = stringResource(Res.string.feature_chat_history)
    val followupLabel = stringResource(Res.string.feature_followup)
    val cancelQuoteLabel = stringResource(Res.string.feature_cancel_quote)
    val attachmentLabel = stringResource(Res.string.feature_attachment)
    val attachmentRemoveLabel = stringResource(Res.string.feature_attachment_remove)
    val attachmentUploadingLabel = stringResource(Res.string.feature_attachment_uploading)
    val attachmentReadyLabel = stringResource(Res.string.feature_attachment_ready)
    val attachmentFailedLabel = stringResource(Res.string.feature_attachment_upload_failed)
    val attachmentRetryLabel = stringResource(Res.string.feature_attachment_retry)
    val attachmentTooLargeLabel = stringResource(Res.string.feature_attachment_too_large)
    val attachmentImageLabel = stringResource(Res.string.chat_composer_attach_image)
    val attachmentScreenshotLabel = stringResource(Res.string.chat_composer_attach_screenshot)
    val mentionLabel = stringResource(Res.string.feature_mention_title)
    val backLabel = stringResource(Res.string.common_back)
    val openGroupLabel = stringResource(Res.string.feature_open_group)
    val mainChatSubtitle = stringResource(Res.string.chat_main_subtitle)
    val botChatSubtitle = stringResource(
        Res.string.chat_bot_subtitle,
        chatBot?.str("label").orEmpty().ifBlank { chatBot?.str("name").orEmpty() },
        activeBotAssignments,
    )
    val chatSubtitle = when (chat?.str("kind")) {
        "main" -> mainChatSubtitle
        "direct", "bot_dm" -> botChatSubtitle
        "project" -> stringResource(Res.string.chat_group_subtitle, memberIds.size)
        else -> chat?.str("attention").orEmpty()
    }
    fun updateAttachment(localId: String, transform: (ChatAttachmentItem) -> ChatAttachmentItem) {
        attachments = attachments.map { item -> if (item.localId == localId) transform(item) else item }
    }
    suspend fun uploadAttachment(item: ChatAttachmentItem, requestHostId: String) {
        updateAttachment(item.localId) { it.copy(status = ChatAttachmentStatus.UPLOADING, error = null) }
        try {
            val source = item.source ?: error(sendError)
            check(source.bytes.isNotEmpty() && source.bytes.size <= MAX_CHAT_ATTACHMENT_BYTES) { attachmentTooLargeLabel }
            val upload = repository.uploadFileOnHost(requestHostId, source)
            if (!isCurrentUploadHost(requestHostId, repository.activeHost.value?.id)) return
            val uploadId = upload.str("upload_id")
            check(uploadId.isNotBlank()) { sendError }
            updateAttachment(item.localId) {
                it.copy(
                    status = ChatAttachmentStatus.READY,
                    uploadId = uploadId,
                    source = null,
                    error = null,
                )
            }
        } catch (cancelled: kotlinx.coroutines.CancellationException) {
            throw cancelled
        } catch (failure: Throwable) {
            if (isCurrentUploadHost(requestHostId, repository.activeHost.value?.id)) {
                updateAttachment(item.localId) {
                    it.copy(status = ChatAttachmentStatus.ERROR, error = failure.message ?: attachmentFailedLabel)
                }
            }
        }
    }
    fun addAttachment(mode: FilePickerMode) {
        if (readOnly) return
        val requestHostId = hostId
        scope.launch {
            try {
                val picked = platformFilePicker().pickFile(mode) ?: return@launch
                if (!isCurrentUploadHost(requestHostId, repository.activeHost.value?.id)) return@launch
                val localId = RandomIdGenerator.nextId()
                val oversized = picked.bytes.size > MAX_CHAT_ATTACHMENT_BYTES
                val preview = if (!oversized && picked.mimeType?.startsWith("image/") == true) {
                    withContext(Dispatchers.Default) { platformScreenImageDecoder().decodeJpeg(picked.bytes) }
                } else null
                val item = ChatAttachmentItem(
                    localId = localId,
                    name = picked.name,
                    size = picked.bytes.size.toLong(),
                    mimeType = picked.mimeType,
                    status = if (oversized) ChatAttachmentStatus.ERROR else ChatAttachmentStatus.UPLOADING,
                    error = if (oversized) attachmentTooLargeLabel else null,
                    source = picked,
                    preview = preview,
                )
                attachments = attachments + item
                if (!oversized) uploadAttachment(item, requestHostId)
            } catch (cancelled: kotlinx.coroutines.CancellationException) {
                throw cancelled
            } catch (failure: Throwable) {
                if (isCurrentUploadHost(requestHostId, repository.activeHost.value?.id)) {
                    error = failure.message ?: attachmentFailedLabel
                }
            }
        }
    }
    val composerPlaceholder = when {
        !state.connected -> stringResource(Res.string.chat_composer_placeholder_offline)
        groupChat -> stringResource(Res.string.chat_composer_placeholder_group, chat?.str("title").orEmpty())
        else -> stringResource(Res.string.chat_composer_placeholder_dm, chat?.str("title").orEmpty().ifBlank { stringResource(Res.string.feature_bot) })
    }
    LaunchedEffect(hostId, chatId) {
        draftLoaded = false
        val store = platformPersistentStore()
        val restoredDraft = parseChatSkillDraft(runCatching { store.read(chatDraftKey(hostId, chatId)) }.getOrNull().orEmpty())
        selectedSkill = restoredDraft.skillName
        draft = restoredDraft.text
        attachments = runCatching { decodeAttachmentDraft(store.read(chatAttachmentsDraftKey(hostId, chatId))) }.getOrDefault(emptyList())
        draftLoaded = true
    }
    LaunchedEffect(chatId, draftLoaded, showHeader, readOnly, historyLoaded, messages.isNotEmpty(), mainBotChat) {
        val canAutoFocus = !mainBotChat || (historyLoaded && messages.isNotEmpty())
        if (draftLoaded && showHeader && !readOnly && canAutoFocus) {
            withFrameNanos { }
            runCatching { composerFocusRequester.requestFocus() }
        }
    }
    LaunchedEffect(draft, selectedSkill, hostId, chatId, draftLoaded) {
        if (draftLoaded) {
            // Drafts update for every IME key; debounce the disk-backed SharedPreferences commit.
            kotlinx.coroutines.delay(400)
            runCatching { platformPersistentStore().write(chatDraftKey(hostId, chatId), serializeChatSkillDraft(selectedSkill, draft)) }
        }
    }
    LaunchedEffect(attachments, hostId, chatId, draftLoaded) {
        if (draftLoaded) {
            kotlinx.coroutines.delay(400)
            runCatching {
                platformPersistentStore().write(chatAttachmentsDraftKey(hostId, chatId), encodeAttachmentDraft(attachments))
            }
        }
    }

    LaunchedEffect(hostId, chatId) {
        historyLoaded = false
        historyHasMore = false
        historyBeforeSeq = null
        scrollState = ChatScrollState()
        runCatching {
            repository.callOnHost(hostId, "chat.history", buildJsonObject { put("chat_id", chatId); put("limit", 100) })

        }.onSuccess { result ->
            historyHasMore = result.boolean("has_more")
            historyBeforeSeq = result.objects("messages").minOfOrNull { it.longValue("seq") ?: Long.MAX_VALUE }
                ?.takeIf { it != Long.MAX_VALUE }
            historyLoaded = true
        }.onFailure { error = it.message ?: loadingError }
            .also { if (it.isFailure) historyLoaded = true }
        // History carries question IDs in its message blocks. Fetch their complete states in one
        // chat-scoped query so answered and expired cards can be reconstructed after reconnect.
        runCatching {
            repository.callOnHost(hostId, "question.list", buildJsonObject { put("chat_id", chatId) })
        }
    }
    LaunchedEffect(hostId, chatId, initialMessageId, historyLoaded) {
        val targetId = initialMessageId?.takeIf(String::isNotBlank) ?: return@LaunchedEffect
        if (!historyLoaded || initialJumpHandled) return@LaunchedEffect
        var hasMore = historyHasMore
        var beforeSeq = historyBeforeSeq
        var pages = 0
        while (repository.state.value.messages[chatId].orEmpty().none { it.str("id") == targetId } && hasMore && beforeSeq != null && pages < 100) {
            val result = runCatching {
                repository.callOnHost(hostId, "chat.history", buildJsonObject {
                    put("chat_id", chatId)
                    put("before_seq", beforeSeq!!)
                    put("limit", 100)
                })
            }.getOrElse { failure ->
                error = failure.message ?: loadingError
                break
            }
            hasMore = result.boolean("has_more")
            beforeSeq = result.objects("messages").minOfOrNull { it.longValue("seq") ?: Long.MAX_VALUE }
                ?.takeIf { it != Long.MAX_VALUE }
            historyHasMore = hasMore
            historyBeforeSeq = beforeSeq
            pages++
        }
        val index = repository.state.value.messages[chatId].orEmpty().indexOfFirst { it.str("id") == targetId }
        if (index >= 0) {
            withFrameNanos { }
            listState.scrollToItem(index + if (historyHasMore) 1 else 0)
            scrollState = scrollState.afterInitialPosition()
            highlightedMessageId = targetId
            kotlinx.coroutines.delay(900)
            if (highlightedMessageId == targetId) highlightedMessageId = null
        } else {
            error = quoteOriginalUnloaded
        }
        initialJumpHandled = true
    }
    val slashPickerOpen = isSlashSkillQuery(draft, selectedSkill)
    val canLoadSlashSkills = shouldLoadSlashSkills(draft, selectedSkill, state.connected)
    LaunchedEffect(hostId, chatId, slashPickerOpen, canLoadSlashSkills) {
        if (slashPickerOpen && canLoadSlashSkills) {
            val loaded = runCatching { repository.callOnHost(hostId, "skill.list", buildJsonObject {}) }
                .getOrNull()?.objects("skills").orEmpty()
            slashSkills = loaded.ifEmpty { repository.state.value.skills }
        } else if (!slashPickerOpen) {
            slashSkills = emptyList()
        }
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
        if (initialMessageId != null && !initialJumpHandled) return@LaunchedEffect
        if (!historyLoaded || scrollState.initialPositioned) return@LaunchedEffect
        val lastIndex = latestChatItemIndex(messages.size, historyHasMore) ?: return@LaunchedEffect
        withFrameNanos { }
        listState.scrollToItem(lastIndex)
        scrollState = scrollState.afterInitialPosition()
    }

    LaunchedEffect(messages.size, messages.lastOrNull()?.toString(), historyHasMore, scrollState) {
        if (initialMessageId != null && !initialJumpHandled) return@LaunchedEffect
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
            FontScaleLimit(1.3f) {
            Row(
                Modifier.fillMaxWidth().heightIn(min = 64.dp).padding(horizontal = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                IconButton(onClick = onBack, modifier = Modifier.semantics { contentDescription = backLabel }) { LucideIcon(LucideIconKind.ArrowLeft, Modifier.size(20.dp)) }
                if (!groupChat && chatBot != null) {
                    BotAvatar(repository, chatBot, main = chatBot.boolean("is_main"), status = chatBot.obj("status")?.str("summary").orEmpty(), size = 40.dp)
                    Spacer(Modifier.width(12.dp))
                }
                Column(Modifier.weight(1f).semantics(mergeDescendants = true) {}, verticalArrangement = Arrangement.spacedBy(1.dp)) {
                    Text(chat?.str("title").orEmpty(), style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(chatSubtitle, style = MaterialTheme.typography.bodySmall, color = LocalMobilePalette.current.textSecondary, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                chat?.str("project_id")?.takeIf { it.isNotBlank() }?.let { projectId ->
                    IconButton(onClick = { onOpenGroupInfo(projectId) }, modifier = Modifier.semantics { contentDescription = groupInfoLabel }) { LucideIcon(LucideIconKind.Info, Modifier.size(20.dp)) }
                }
                if (chat?.str("kind") == "bot_dm") {
                    IconButton(onClick = { onOpenScreen(chat?.str("bot_id").orEmpty(), null) }) {
                        LucideIcon(LucideIconKind.Monitor, Modifier.size(20.dp))
                    }
                }
                if (chat?.str("kind") in setOf("main", "direct", "bot_dm")) {
                    IconButton(onClick = { onOpenFollowup(chatId) }, modifier = Modifier.semantics { contentDescription = followupLabel }) {
                        LucideIcon(LucideIconKind.PanelRight, Modifier.size(20.dp))
                    }
                }
            }
            }
            HorizontalDivider()
        }
        val pinTitle = when {
            waitingApproval != null -> stringResource(Res.string.chat_pin_pending_approval, 1)
            waitingQuestion != null -> stringResource(Res.string.chat_pin_waiting_you, 1)
            waitingReviewProject != null -> stringResource(Res.string.chat_pin_waiting_you, 1)
            else -> null
        }
        val pinSummary = when {
            waitingApproval != null -> "${waitingApproval.str("tool").ifBlank { waitingApproval.str("summary") }} · ${waitingApproval.str("summary")}"
            waitingQuestion != null -> waitingQuestion.str("text")
            waitingReviewProject != null -> "${waitingReviewProject.str("name")} · ${stringResource(Res.string.status_review_you)}"
            else -> null
        }
        LaunchedEffect(pinTitle != null, scrollState.initialPositioned) {
            if (pinTitle != null && scrollState.initialPositioned) {
                // A pending banner can arrive after chat history has already scrolled. The
                // reduced viewport then leaves the first visible message clipped at its top.
                // Keep the same message in view, but align its top below the banner.
                withFrameNanos { }
                listState.scrollToItem(listState.firstVisibleItemIndex)
            }
        }
        if (pinTitle != null && pinSummary != null) {
            Row(
                Modifier.fillMaxWidth().heightIn(min = 48.dp)
                    .background(if (waitingApproval != null || waitingQuestion != null) LocalMobilePalette.current.warningSoft else LocalMobilePalette.current.reviewSoft)
                    .clickable { onOpenFollowup(chatId) }
                    .padding(horizontal = 14.dp, vertical = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                LucideIcon(
                    if (waitingApproval != null || waitingQuestion != null) LucideIconKind.ShieldAlert else LucideIconKind.Target,
                    Modifier.size(20.dp),
                    tint = if (waitingApproval != null || waitingQuestion != null) LocalMobilePalette.current.warning else LocalMobilePalette.current.review,
                )
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(1.dp)) {
                    Text(pinTitle, style = MaterialTheme.typography.labelLarge, maxLines = 1)
                    Text(pinSummary, style = MaterialTheme.typography.bodySmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                LucideIcon(LucideIconKind.ChevronRight, Modifier.size(18.dp), tint = LocalMobilePalette.current.textSecondary)
            }
        }
        FontScaleLimit(2.0f) {
        LazyColumn(
            state = listState,
            modifier = Modifier.weight(1f).fillMaxWidth().padding(horizontal = 12.dp),
            verticalArrangement = Arrangement.spacedBy(0.dp),
        ) {
            if (showMainChatEmptyState) {
                item(key = "main-chat-empty-state") {
                    MainBotEmptyState(
                        onSuggestion = { suggestion ->
                            applyMainBotSuggestion(
                                suggestion,
                                setDraft = { draft = it },
                                requestFocus = {
                                    scope.launch {
                                        withFrameNanos { }
                                        runCatching { composerFocusRequester.requestFocus() }
                                    }
                                },
                            )
                        },
                    )
                }
            }
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
                    ) { Text(if (loadingOlder) stringResource(Res.string.feature_loading) else stringResource(Res.string.chat_loading_older)) }
                }
            }
            itemsIndexed(messages, key = { _, message -> "$hostId:$chatId:${message.str("id").ifBlank { "message:${message.hashCode()}" }}" }) { messageIndex, message ->
                val previousMessage = messages.getOrNull(messageIndex - 1)
                val nextMessage = messages.getOrNull(messageIndex + 1)
                val startsNewDay = previousMessage == null || !sameChatDay(previousMessage, message)
                val hasTimeSeparator = previousMessage != null && hasChatTimeSeparator(previousMessage, message)
                if (startsNewDay) ChatDayCapsule(message.str("created_at"))
                if (hasTimeSeparator) ChatTimeSeparator(message.str("created_at"))
                val followsSameAuthor = previousMessage?.let { sameMessageGroup(it, message) } == true && !hasTimeSeparator
                val continuesSameAuthor = nextMessage?.let { sameMessageGroup(message, it) } == true && !hasChatTimeSeparator(message, nextMessage)
                val messageTopSpacing = when {
                    startsNewDay || hasTimeSeparator -> 0.dp
                    followsSameAuthor -> 2.dp
                    else -> 10.dp
                }
                ChatMessageRow(
                    message = message,
                    hostId = hostId,
                    chatId = chatId,
                    repository = repository,
                    onQuoteAnnouncement = onQuoteAnnouncement,
                    showSender = !followsSameAuthor,
                    showAvatar = !continuesSameAuthor,
                    followsSameAuthor = followsSameAuthor,
                    continuesSameAuthor = continuesSameAuthor,
                    showTime = !hasTimeSeparator,
                    showExactTime = exactTimeMessageKey == "$hostId:$chatId:${message.str("id")}",
                    onExactTimeReveal = { reveal ->
                        val messageKey = "$hostId:$chatId:${message.str("id")}"
                        if (reveal) exactTimeMessageKey = messageKey
                        else if (exactTimeMessageKey == messageKey) exactTimeMessageKey = null
                    },
                    lastOwnMessage = messageIndex == lastOwnMessageIndex,
                    topSpacing = messageTopSpacing,
                    onJumpToMessage = { quotedId ->
                        val index = messages.indexOfFirst { it.str("id") == quotedId }
                        if (index >= 0) {
                            scope.launch {
                                val targetIndex = index + if (historyHasMore) 1 else 0
                                if (animationsEnabled) {
                                    listState.animateScrollToItem(targetIndex)
                                    highlightedMessageId = quotedId
                                    kotlinx.coroutines.delay(600)
                                    if (highlightedMessageId == quotedId) highlightedMessageId = null
                                } else {
                                    listState.scrollToItem(targetIndex)
                                }
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
                                    val image = platformScreenImageDecoder().decodeJpeg(repository.fetchBytesOnHost(hostId, "/api/v1/files", params))
                                        ?: throw IllegalStateException("Unable to decode image")
                                    imagePreviews = imagePreviews + (fileKey(ref) to image)
                                } else {
                                    filePreviews = filePreviews + (fileKey(ref) to repository.fetchTextOnHost(hostId, "/api/v1/files", params))
                                }
                            }.onFailure { error = it.message ?: sendError }
                        }
                    },
                    onExportFile = { file ->
                        scope.launch {
                            val ref = fileRef(file)
                            runCatching { repository.fetchBytesOnHost(hostId, "/api/v1/files", fileParams(ref)) }
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
        }
        replyTo?.let { id ->
            Surface(Modifier.fillMaxWidth(), color = MaterialTheme.colorScheme.surfaceContainerHigh) {
                Row(Modifier.padding(horizontal = 12.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                    val quoted = messages.firstOrNull { it.str("id") == id }
                    val sender = quoted?.obj("sender")?.str("bot_id")?.let { botId -> state.bots.firstOrNull { it.str("id") == botId }?.str("name") }
                        ?.takeIf(String::isNotBlank) ?: stringResource(Res.string.common_you)
                    Spacer(Modifier.width(2.dp).heightIn(min = 36.dp).background(MaterialTheme.colorScheme.primary))
                    Column(Modifier.weight(1f).padding(horizontal = 8.dp)) {
                        Text(
                            stringResource(if (quoteMode == "reference") Res.string.chat_quote_bar_title else Res.string.chat_reply_bar_title, sender),
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
                    IconButton(onClick = { replyTo = null; quoteMode = "reply" }, modifier = Modifier.semantics { contentDescription = cancelQuoteLabel }) { LucideIcon(LucideIconKind.X, Modifier.size(18.dp)) }
                }
            }
        }
        error?.let { Text(it, Modifier.padding(horizontal = 12.dp), color = MaterialTheme.colorScheme.error) }
        if (attachments.isNotEmpty()) Row(
            Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 12.dp, vertical = 4.dp),
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            attachments.forEach { item ->
                Surface(color = MaterialTheme.colorScheme.surfaceContainerHigh, shape = RoundedCornerShape(18.dp)) {
                    Row(Modifier.padding(start = 8.dp, end = 2.dp, top = 4.dp, bottom = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                        if (item.preview != null) {
                            Image(
                                bitmap = item.preview,
                                contentDescription = null,
                                modifier = Modifier.size(36.dp).clip(RoundedCornerShape(8.dp)),
                                contentScale = ContentScale.Crop,
                            )
                        } else {
                            LucideIcon(LucideIconKind.FileText, Modifier.size(20.dp).padding(2.dp), tint = MaterialTheme.colorScheme.primary)
                        }
                        Column(Modifier.padding(start = 6.dp).widthIn(max = 180.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                            Text(item.name, style = MaterialTheme.typography.labelMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            val status = when (item.status) {
                                ChatAttachmentStatus.UPLOADING -> attachmentUploadingLabel
                                ChatAttachmentStatus.READY -> "${fileSizeLabel(item.size)} · $attachmentReadyLabel"
                                ChatAttachmentStatus.ERROR -> item.error ?: attachmentFailedLabel
                            }
                            Text(status, style = MaterialTheme.typography.labelSmall, color = if (item.status == ChatAttachmentStatus.ERROR) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            if (item.status == ChatAttachmentStatus.UPLOADING) LinearProgressIndicator(Modifier.fillMaxWidth())
                        }
                        if (item.status == ChatAttachmentStatus.ERROR && item.source?.bytes?.size?.let { it <= MAX_CHAT_ATTACHMENT_BYTES } == true) IconButton(
                            onClick = { item.source?.let { source -> scope.launch { uploadAttachment(item.copy(source = source), hostId) } } },
                            modifier = Modifier.size(36.dp).semantics { contentDescription = "$attachmentRetryLabel ${item.name}" },
                        ) { LucideIcon(LucideIconKind.RefreshCw, Modifier.size(16.dp)) }
                        IconButton(
                            enabled = item.status != ChatAttachmentStatus.UPLOADING,
                            onClick = { attachments = attachments.filterNot { it.localId == item.localId } },
                            modifier = Modifier.size(36.dp).semantics { contentDescription = "$attachmentRemoveLabel ${item.name}" },
                        ) { LucideIcon(LucideIconKind.X, Modifier.size(16.dp)) }
                    }
                }
            }
        }
        val slashQuery = draft.substringAfter('/', "").substringBefore(' ')
        if (slashPickerOpen) {
            Surface(
                Modifier.fillMaxWidth().padding(horizontal = 12.dp).padding(bottom = 4.dp),
                color = LocalMobilePalette.current.elevated,
                shape = RoundedCornerShape(topStart = 12.dp, topEnd = 12.dp, bottomStart = 4.dp, bottomEnd = 4.dp),
                shadowElevation = 8.dp,
                border = androidx.compose.foundation.BorderStroke(1.dp, LocalMobilePalette.current.borderDefault),
            ) {
                Column(Modifier.heightIn(max = 340.dp).verticalScroll(rememberScrollState()).padding(6.dp)) {
                    val availableSkills = filterSlashSkills(slashSkills.ifEmpty { state.skills }, "")
                    val matches = filterSlashSkills(availableSkills, slashQuery)
                    if (slashQuery.isBlank()) {
                        val recent = recentSlashSkills(availableSkills, Clock.System.now())
                        if (recent.isNotEmpty()) {
                            Text(stringResource(Res.string.chat_slash_recent), Modifier.padding(horizontal = 10.dp, vertical = 4.dp), style = MaterialTheme.typography.labelMedium, color = LocalMobilePalette.current.textSecondary)
                            recent.forEach { skill ->
                                SlashSkillOption(skill, onSelect = { name ->
                                    selectedSkill = name.takeIf { it.isNotBlank() }
                                    draft = ""
                                    error = null
                                    scope.launch { withFrameNanos { }; runCatching { composerFocusRequester.requestFocus() } }
                                })
                            }
                        }
                        Text(stringResource(Res.string.chat_slash_all, matches.size), Modifier.padding(horizontal = 10.dp, vertical = 4.dp), style = MaterialTheme.typography.labelMedium, color = LocalMobilePalette.current.textSecondary)
                    } else {
                        Text(stringResource(Res.string.chat_slash_matched, slashQuery, matches.size), Modifier.padding(horizontal = 10.dp, vertical = 4.dp), style = MaterialTheme.typography.labelMedium, color = LocalMobilePalette.current.textSecondary)
                    }
                    matches.forEach { skill ->
                        SlashSkillOption(skill, onSelect = { name ->
                            selectedSkill = name.takeIf { it.isNotBlank() }
                            draft = ""
                            error = null
                            scope.launch { withFrameNanos { }; runCatching { composerFocusRequester.requestFocus() } }
                        })
                    }
                    if (matches.isEmpty()) {
                        Column(Modifier.fillMaxWidth().padding(12.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(6.dp)) {
                            Text(stringResource(Res.string.feature_skill_suggestions_empty), color = MaterialTheme.colorScheme.onSurfaceVariant)
                            Text(stringResource(Res.string.chat_slash_empty_desc), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                            TextButton(onClick = { onCreateSkill(slashQuery) }, enabled = slashQuery.isNotBlank()) {
                                Text(stringResource(Res.string.chat_slash_create, slashQuery))
                            }
                        }
                    }
                }
            }
        }
        if (workingProjectBots.isNotEmpty() && mentionBots.isEmpty() && '@' !in draft) {
            Column(
                Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp),
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                Text(
                    stringResource(Res.string.feature_group_message_queued_hint),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                )
                Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    workingProjectBots.forEach { bot ->
                        val botId = bot.str("id")
                        val botName = bot.str("name").ifBlank { botId }
                        TextButton(
                            onClick = {
                                mentionBots = mentionBots + botId
                                draft = "@$botName ${draft.trimStart()}"
                            },
                            contentPadding = PaddingValues(horizontal = 10.dp, vertical = 0.dp),
                        ) {
                            Text(stringResource(Res.string.chat_queue_at_now, botName), maxLines = 1)
                        }
                    }
                }
            }
        }
        Row(Modifier.fillMaxWidth().padding(10.dp), verticalAlignment = Alignment.Bottom) {
            IconButton(enabled = !readOnly, onClick = { showAttachmentMenu = true }, modifier = Modifier.semantics { contentDescription = attachmentLabel }) { LucideIcon(LucideIconKind.Plus, Modifier.size(20.dp)) }
        Column(Modifier.weight(1f), horizontalAlignment = Alignment.Start) {
            selectedSkill?.let { skillName ->
                val removeSkillDescription = stringResource(Res.string.common_close)
                Surface(
                    color = LocalMobilePalette.current.accentSoft,
                    contentColor = LocalMobilePalette.current.accent,
                    shape = RoundedCornerShape(8.dp),
                    modifier = Modifier.padding(bottom = 4.dp).widthIn(max = 220.dp),
                ) {
                    Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(start = 8.dp, end = 2.dp)) {
                        LucideIcon(LucideIconKind.Zap, Modifier.size(14.dp))
                        Text(skillName, Modifier.weight(1f).padding(start = 4.dp), style = MaterialTheme.typography.labelMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        IconButton(
                            onClick = { selectedSkill = null },
                            modifier = Modifier.size(30.dp).semantics { contentDescription = removeSkillDescription },
                        ) { LucideIcon(LucideIconKind.X, Modifier.size(14.dp)) }
                    }
                }
            }
            OutlinedTextField(
                value = draft,
                onValueChange = { draft = it; if (it.endsWith("@")) showMentions = true },
                modifier = Modifier.fillMaxWidth().focusRequester(composerFocusRequester).onPreviewKeyEvent { event ->
                    if (event.type == KeyEventType.KeyDown && event.key == Key.Backspace && draft.isEmpty() && selectedSkill != null) {
                        selectedSkill = null
                        true
                    } else false
                },
                placeholder = { Text(composerPlaceholder) },
                maxLines = 4,
            )
        }
            if (groupChat || mentionCandidates.isNotEmpty()) IconButton(
                onClick = { mentionOpenedByButton = true; showMentions = true },
                modifier = Modifier.focusRequester(mentionButtonFocusRequester).semantics { contentDescription = mentionLabel },
            ) { LucideIcon(LucideIconKind.AtSign, Modifier.size(20.dp), tint = LocalMobilePalette.current.accent) }
            Spacer(Modifier.padding(4.dp))
            Button(
                enabled = canSendChatMessage(serializeChatMessageText(selectedSkill, draft), attachments, readOnly),
                onClick = {
                    val text = serializeChatMessageText(selectedSkill, draft)
                    val uploadIds = attachments.mapNotNull { it.uploadId }
                    val sendHostId = hostId
                    scope.launch {
                        runCatching {
                            repository.callOnHost(sendHostId, "chat.send", buildJsonObject {
                                put("chat_id", chatId)
                                put("text", text)
                                put("mentions", buildJsonArray {
                                    mentionBots.forEach { botId ->
                                        if (botId == "__everyone__") add(buildJsonObject { put("kind", "everyone") })
                                        else add(buildJsonObject { put("kind", "bot"); put("bot_id", botId); put("instruction", JsonNull) })
                                    }
                                })
                                put("attachments", buildJsonArray { uploadIds.forEach { add(JsonPrimitive(it)) } })
                                replyTo?.let { put("quote", it); put("quote_mode", quoteMode) }
                            }.withClientLocale(platformLocaleTag()))
                        }.onSuccess {
                            if (isCurrentUploadHost(sendHostId, repository.activeHost.value?.id)) {
                                draft = ""
                                selectedSkill = null
                                replyTo = null
                                quoteMode = "reply"
                                mentionBots = emptySet()
                                attachments = emptyList()
                                error = null
                            }
                        }.onFailure {
                            if (isCurrentUploadHost(sendHostId, repository.activeHost.value?.id)) error = it.message ?: sendError
                        }
                    }
                },
            ) { Text(if (readOnly) stringResource(Res.string.feature_readonly) else stringResource(Res.string.common_send)) }
        }
        if (showAttachmentMenu) androidx.compose.material3.ModalBottomSheet(onDismissRequest = { showAttachmentMenu = false }) {
            Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                TextButton(onClick = { showAttachmentMenu = false; addAttachment(FilePickerMode.FILE) }, modifier = Modifier.fillMaxWidth()) {
                    Text(stringResource(Res.string.chat_composer_attach_file), modifier = Modifier.fillMaxWidth())
                }
                TextButton(onClick = { showAttachmentMenu = false; addAttachment(FilePickerMode.IMAGE) }, modifier = Modifier.fillMaxWidth()) {
                    Text(attachmentImageLabel, modifier = Modifier.fillMaxWidth())
                }
                TextButton(onClick = { showAttachmentMenu = false; addAttachment(FilePickerMode.SCREENSHOT) }, modifier = Modifier.fillMaxWidth()) {
                    Text(attachmentScreenshotLabel, modifier = Modifier.fillMaxWidth())
                }
            }
        }
        if (showMentions) androidx.compose.material3.ModalBottomSheet(onDismissRequest = {
            showMentions = false
            if (mentionOpenedByButton) scope.launch {
                withFrameNanos { }
                runCatching { mentionButtonFocusRequester.requestFocus() }
            }
            mentionOpenedByButton = false
        }) {
            Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(stringResource(Res.string.chat_mention_title), style = MaterialTheme.typography.titleMedium)
                if (groupChat) TextButton(onClick = {
                    mentionOpenedByButton = false
                    mentionBots = mentionBots + "__everyone__"
                    draft = draft.removeSuffix("@") + "@everyone "
                    showMentions = false
                    scope.launch { withFrameNanos { }; runCatching { composerFocusRequester.requestFocus() } }
                }) { Text(stringResource(Res.string.chat_mention_everyone)) }
                mentionCandidates.forEach { bot ->
                    val id = bot.str("id")
                    TextButton(onClick = {
                        mentionOpenedByButton = false
                        mentionBots = mentionBots + id
                        draft = draft.removeSuffix("@") + "@${bot.str("name")} "
                        showMentions = false
                        scope.launch { withFrameNanos { }; runCatching { composerFocusRequester.requestFocus() } }
                    }) {
                        Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.Start) {
                            Text(bot.str("name"), style = MaterialTheme.typography.titleSmall)
                            Text(
                                if (bot.obj("status").str("summary") in setOf("working", "queued")) stringResource(Res.string.chat_mention_steer_hint) else stringResource(Res.string.feature_idle),
                                style = MaterialTheme.typography.bodySmall,
                            )
                        }
                    }
                }
            }
        }
    }
}

private fun sameMessageGroup(first: JsonObject, second: JsonObject): Boolean {
    val firstSender = first.obj("sender")
    val secondSender = second.obj("sender")
    if (firstSender.str("kind") == "system" || secondSender.str("kind") == "system") return false
    if (hasTimelineBreak(first) || hasTimelineBreak(second)) return false
    if (firstSender.str("kind") != secondSender.str("kind") || firstSender.str("bot_id") != secondSender.str("bot_id")) return false
    val zone = TimeZone.currentSystemDefault()
    val firstInstant = runCatching { Instant.parse(first.str("created_at")) }.getOrNull() ?: return false
    val secondInstant = runCatching { Instant.parse(second.str("created_at")) }.getOrNull() ?: return false
    if (firstInstant.toLocalDateTime(zone).date != secondInstant.toLocalDateTime(zone).date) return false
    return abs(firstInstant.toEpochMilliseconds() - secondInstant.toEpochMilliseconds()) < 5 * 60 * 1000L
}

@Composable
private fun ChatDayCapsule(timestamp: String) {
    val instant = runCatching { Instant.parse(timestamp) }.getOrNull() ?: return
    val zone = TimeZone.currentSystemDefault()
    val local = instant.toLocalDateTime(zone)
    val today = Clock.System.now().toLocalDateTime(zone)
    val dayDifference = today.date.toEpochDays() - local.date.toEpochDays()
    val label = when {
        dayDifference == 0L -> stringResource(Res.string.chat_day_today)
        dayDifference == 1L -> stringResource(Res.string.chat_time_yesterday)
        dayDifference in 2L..7L -> {
            val weekday = when (local.dayOfWeek.ordinal + 1) {
                1 -> stringResource(Res.string.chat_weekday_mon)
                2 -> stringResource(Res.string.chat_weekday_tue)
                3 -> stringResource(Res.string.chat_weekday_wed)
                4 -> stringResource(Res.string.chat_weekday_thu)
                5 -> stringResource(Res.string.chat_weekday_fri)
                6 -> stringResource(Res.string.chat_weekday_sat)
                else -> stringResource(Res.string.chat_weekday_sun)
            }
            stringResource(Res.string.chat_time_weekday, weekday)
        }
        local.year == today.year -> stringResource(Res.string.chat_time_date, padChatTimePart(local.month.ordinal + 1), padChatTimePart(local.day))
        else -> stringResource(Res.string.chat_time_date_year, local.year, padChatTimePart(local.month.ordinal + 1), padChatTimePart(local.day))
    }
    val palette = LocalMobilePalette.current
    Row(Modifier.fillMaxWidth().padding(vertical = 12.dp), horizontalArrangement = Arrangement.Center) {
        Surface(
            color = palette.panel,
            shape = RoundedCornerShape(11.dp),
            border = androidx.compose.foundation.BorderStroke(1.dp, palette.borderDefault),
        ) {
            Text(label, Modifier.padding(horizontal = 10.dp, vertical = 3.dp), style = MaterialTheme.typography.labelSmall, color = palette.textSecondary)
        }
    }
}

@Composable
private fun ChatTimeSeparator(timestamp: String) {
    val instant = runCatching { Instant.parse(timestamp) }.getOrNull() ?: return
    val local = instant.toLocalDateTime(TimeZone.currentSystemDefault())
    Text(
        "${padChatTimePart(local.hour)}:${padChatTimePart(local.minute)}",
        Modifier.fillMaxWidth().padding(top = 10.dp, bottom = 4.dp),
        style = MaterialTheme.typography.labelSmall,
        color = LocalMobilePalette.current.textTertiary,
        textAlign = androidx.compose.ui.text.style.TextAlign.Center,
    )
}

@Composable
private fun ChatClockText(timestamp: String, modifier: Modifier = Modifier) {
    val instant = runCatching { Instant.parse(timestamp) }.getOrNull()
    val label = instant?.toLocalDateTime(TimeZone.currentSystemDefault())?.let { "${padChatTimePart(it.hour)}:${padChatTimePart(it.minute)}" }
        ?: timestamp.substringAfter('T').take(5)
    Text(label, modifier, style = MaterialTheme.typography.labelSmall, color = LocalMobilePalette.current.textTertiary)
}

private fun sameChatDay(first: JsonObject, second: JsonObject): Boolean {
    val zone = TimeZone.currentSystemDefault()
    val firstDate = runCatching { Instant.parse(first.str("created_at")).toLocalDateTime(zone).date }.getOrNull() ?: return false
    val secondDate = runCatching { Instant.parse(second.str("created_at")).toLocalDateTime(zone).date }.getOrNull() ?: return false
    return firstDate == secondDate
}

private fun hasChatTimeSeparator(first: JsonObject, second: JsonObject): Boolean {
    if (!sameChatDay(first, second)) return false
    val firstInstant = runCatching { Instant.parse(first.str("created_at")) }.getOrNull() ?: return false
    val secondInstant = runCatching { Instant.parse(second.str("created_at")) }.getOrNull() ?: return false
    return abs(secondInstant.toEpochMilliseconds() - firstInstant.toEpochMilliseconds()) >= 5 * 60 * 1000L
}

private fun hasTimelineBreak(message: JsonObject): Boolean =
    message.arr("blocks").any { element ->
        val type = (element as? JsonObject)?.str("type")
        type == "task_card" || type == "system" || type == "progress"
    }

internal fun padChatTimePart(value: Int): String = value.toString().padStart(2, '0')

private data class ScrollAnchor(val id: String, val offset: Int)

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun ChatMessageRow(
    message: JsonObject,
    hostId: String,
    chatId: String,
    repository: MobileRepository,
    onQuoteAnnouncement: (String) -> Unit,
    showSender: Boolean,
    showAvatar: Boolean,
    followsSameAuthor: Boolean,
    continuesSameAuthor: Boolean,
    showTime: Boolean,
    showExactTime: Boolean,
    onExactTimeReveal: (Boolean) -> Unit,
    lastOwnMessage: Boolean,
    topSpacing: androidx.compose.ui.unit.Dp,
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
    val messageIdentity = "$hostId:$chatId:${message.str("id")}"
    var showMessageActions by remember(messageIdentity) { mutableStateOf(false) }
    var messageActionError by remember(messageIdentity) { mutableStateOf(false) }
    val messageFocusRequester = remember(messageIdentity) { FocusRequester() }
    val actionScope = rememberCoroutineScope()
    var exactTimeHideJob by remember(messageIdentity) { mutableStateOf<Job?>(null) }
    fun dismissMessageActions() {
        showMessageActions = false
        actionScope.launch {
            withFrameNanos { }
            runCatching { messageFocusRequester.requestFocus() }
        }
    }
    val haptics = LocalHapticFeedback.current
    val clipboard = LocalClipboardManager.current
    val swipeThreshold = with(androidx.compose.ui.platform.LocalDensity.current) { 56.dp.toPx() }
    var didSwipeReply by remember(messageIdentity) { mutableStateOf(false) }
    var exactTimeOffsetPx by remember(messageIdentity) { mutableStateOf(0f) }
    val animatedExactTimeOffsetPx by animateFloatAsState(
        targetValue = exactTimeOffsetPx,
        animationSpec = tween(durationMillis = 200),
        label = "messageExactTimeReveal",
    )
    val sender = message.obj("sender")
    val isUser = sender.str("kind") == "user"
    val id = message.str("id")
    val blocks = message.arr("blocks")
    val assignmentId = message.str("assignment_id")
    val senderId = sender.str("bot_id")
    val senderLabel = state.bots.firstOrNull { it.str("id")==senderId }?.str("name")?.takeIf { it.isNotBlank() } ?: if (senderId.isBlank()) stringResource(Res.string.feature_bot) else senderId
    val senderBot = state.bots.firstOrNull { it.str("id") == senderId }
    val senderStatus = senderBot?.obj("status")?.str("summary").orEmpty()
    val avatarStatusLabel = stringResource(when (senderStatus) {
        "working", "queued" -> Res.string.chat_avatar_status_working
        "waiting_user" -> Res.string.chat_avatar_status_waiting_user
        "review" -> Res.string.chat_avatar_status_review
        "blocked" -> Res.string.chat_avatar_status_blocked
        else -> Res.string.chat_avatar_status_idle
    })
    val avatarDescription = stringResource(Res.string.chat_avatar_description, senderLabel, avatarStatusLabel)
    val messageInstant = runCatching { Instant.parse(message.str("created_at")) }.getOrNull()
    val messageLocalTime = messageInstant?.toLocalDateTime(TimeZone.currentSystemDefault())
    val spokenTime = if (messageLocalTime == null) message.str("created_at") else stringResource(
        Res.string.chat_time_full,
        messageLocalTime.year,
        padChatTimePart(messageLocalTime.month.ordinal + 1),
        padChatTimePart(messageLocalTime.day),
        padChatTimePart(messageLocalTime.hour),
        padChatTimePart(messageLocalTime.minute),
        padChatTimePart(messageLocalTime.second),
    )
    val blockText = blocks.mapNotNull { entry ->
        (entry as? JsonObject)?.takeIf { it.str("type") == "text" }?.str("markdown")?.takeIf(String::isNotBlank)
    }.joinToString(" ")
    val messageBody = blockText.ifBlank { message.str("fallback_text") }
    val messageCopyText = blocks.mapNotNull { entry ->
        (entry as? JsonObject)?.takeIf { it.str("type") == "text" }?.str("markdown")?.takeIf(String::isNotBlank)
    }.joinToString("\n\n").ifBlank { message.str("fallback_text") }
    val loadedNames = if (isUser) loadedSkillNames(message) else emptyList()
    val enabledSkillNames = state.skills.asSequence()
        .filter { it.boolean("enabled") }
        .map { it.str("name") }
        .filter(String::isNotBlank)
        .toSet()
    val messageSkillPrefix = if (isUser) resolveChatMessageSkillPrefix(messageBody, enabledSkillNames, loadedNames.toSet()) else null
    val visibleSkillNames = (listOfNotNull(messageSkillPrefix?.skillName) + loadedNames).distinct()
    val firstTextBlockIndex = if (messageSkillPrefix == null) -1 else blocks.indexOfFirst { entry ->
        (entry as? JsonObject)?.let { it.str("type") == "text" && it.str("markdown").isNotBlank() } == true
    }
    val displayBlocks = if (firstTextBlockIndex < 0) blocks else blocks.mapIndexed { index, entry ->
        if (index != firstTextBlockIndex) entry else {
            val block = entry as? JsonObject ?: return@mapIndexed entry
            JsonObject(block + ("markdown" to JsonPrimitive(messageSkillPrefix?.body.orEmpty())))
        }
    }
    val spokenBody = messageBody.ifBlank { if (blocks.isNotEmpty()) stringResource(Res.string.chat_message_accessibility_card) else "" }
    val messageDescription = stringResource(Res.string.chat_message_accessibility, if (isUser) stringResource(Res.string.common_you) else senderLabel, spokenTime, spokenBody)
    val standaloneTask = blocks.singleOrNull()?.let { it as? JsonObject }?.takeIf { it.str("type") == "task_card" }
    val actionAssignmentId = assignmentId.ifBlank { standaloneTask?.str("assignment_id").orEmpty() }
    Column(Modifier.fillMaxWidth().padding(top = topSpacing).semantics { isTraversalGroup = true; contentDescription = messageDescription }, horizontalAlignment = if (isUser) Alignment.End else Alignment.Start) {
        if (standaloneTask != null) {
            StandaloneTaskCard(standaloneTask, state, onOpenTrace) {
                showMessageActions = true
            }
        } else {
        if (showSender && !isUser) {
            Row(Modifier.padding(start = 4.dp, bottom = 3.dp), horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(senderLabel, style = MaterialTheme.typography.labelMedium, color = palette.textPrimary)
                senderBot?.str("role_note")?.takeIf { it.isNotBlank() }?.let { Text(it, style = MaterialTheme.typography.labelSmall, color = palette.textTertiary) }
                if (showTime) ChatClockText(message.str("created_at"))
            }
        }
        Box(Modifier.fillMaxWidth()) {
        if (showExactTime) {
            Row(Modifier.align(Alignment.CenterEnd).padding(end = 2.dp), verticalAlignment = Alignment.CenterVertically) {
                ChatClockText(message.str("created_at"))
            }
        }
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.Bottom, horizontalArrangement = if (isUser) Arrangement.End else Arrangement.Start) {
        if (!isUser) {
            if (showAvatar) Box(Modifier.semantics { contentDescription = avatarDescription }) {
                BotAvatar(repository, senderBot, main = senderBot?.boolean("is_main") == true, status = senderStatus, size = 32.dp)
            }
            else Spacer(Modifier.size(32.dp))
        }
        val bubbleShape = RoundedCornerShape(18.dp)
        val bubbleBrush = Brush.verticalGradient(
            listOf(
                palette.tokenColors["bubble.out.top"] ?: palette.bubbleOut,
                palette.bubbleOut,
            ),
        )
        Surface(
            modifier = (if (isUser) Modifier.fillMaxWidth(0.9f) else Modifier.weight(1f).padding(start = 8.dp))
                .shadow(1.dp, bubbleShape, ambientColor = palette.borderSubtle, spotColor = palette.borderSubtle)
                .drawBehind {
                    if (!continuesSameAuthor) {
                        val tailWidth = 18.dp.toPx()
                        val tailHeight = 6.dp.toPx()
                        val path = Path().apply {
                            if (isUser) {
                                moveTo(size.width - tailWidth, size.height - 2.dp.toPx())
                                cubicTo(size.width - 10.dp.toPx(), size.height + tailHeight, size.width - 7.dp.toPx(), size.height + tailHeight, size.width, size.height - 1.dp.toPx())
                                lineTo(size.width - tailWidth, size.height - 2.dp.toPx())
                            } else {
                                moveTo(0f, size.height - 2.dp.toPx())
                                cubicTo(10.dp.toPx(), size.height + tailHeight, 7.dp.toPx(), size.height + tailHeight, 0f, size.height - 1.dp.toPx())
                                lineTo(0f, size.height - 2.dp.toPx())
                            }
                            close()
                        }
                        drawPath(path, if (isUser) palette.bubbleOut else palette.bubbleIn)
                    }
                }
                .background(if (isUser) bubbleBrush else Brush.verticalGradient(listOf(palette.bubbleIn, palette.bubbleIn)), bubbleShape)
                .focusRequester(messageFocusRequester)
                .offset { IntOffset(-animatedExactTimeOffsetPx.roundToInt(), 0) }
                .pointerInput(message.str("id"), swipeThreshold) {
                    var distance = 0f
                    detectHorizontalDragGestures(
                        onDragStart = { exactTimeHideJob?.cancel() },
                        onHorizontalDrag = { change, amount ->
                            distance += amount
                            if (distance < 0f) {
                                exactTimeOffsetPx = (-distance).coerceAtMost(swipeThreshold)
                                onExactTimeReveal(true)
                            }
                            change.consume()
                        },
                        onDragEnd = {
                            if (distance >= swipeThreshold && id.isNotBlank()) {
                                didSwipeReply = true
                                haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                                onReply(id)
                            }
                            exactTimeOffsetPx = 0f
                            exactTimeHideJob?.cancel()
                            if (distance < 0f) {
                                exactTimeHideJob = actionScope.launch {
                                    delay(200)
                                    onExactTimeReveal(false)
                                }
                            } else {
                                onExactTimeReveal(false)
                            }
                            distance = 0f
                        },
                        onDragCancel = { exactTimeHideJob?.cancel(); onExactTimeReveal(false); exactTimeOffsetPx = 0f; distance = 0f },
                    )
                }
                .combinedClickable(
                    onClick = { if (didSwipeReply) didSwipeReply = false },
                    onLongClick = {
                        haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                        showMessageActions = true
                    },
                    onDoubleClick = { if (id.isNotBlank()) onReact(id, "👍") },
                ),
            shape = bubbleShape,
            color = if (isUser) androidx.compose.ui.graphics.Color.Transparent else palette.bubbleIn,
            contentColor = if (isUser) palette.bubbleOutText else palette.bubbleInText,
            border = when {
                highlighted -> BorderStroke(2.dp, MaterialTheme.colorScheme.primary)
                isUser -> null
                else -> BorderStroke(0.5.dp, palette.borderSubtle)
            },
        ) {
            Column(
                Modifier.fillMaxWidth(0.9f).padding(horizontal = 12.dp, vertical = 8.dp),
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
                                ?.takeIf { it.isNotBlank() } ?: stringResource(Res.string.common_you)
                            Text(quoteSender, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.primary)
                            Text(quote.str("excerpt"), style = MaterialTheme.typography.bodySmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            }
                        }
                    }
                }
                if (message.boolean("streaming")) Text(stringResource(Res.string.chat_typing), style = MaterialTheme.typography.labelMedium)
                if (visibleSkillNames.isNotEmpty()) Row(
                    Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()),
                    horizontalArrangement = Arrangement.spacedBy(6.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    visibleSkillNames.forEach { skillName ->
                        MessageSkillChip(skillName)
                        if (skillName in loadedNames) {
                            Text(
                                stringResource(Res.string.chat_slash_loaded, skillName),
                                style = MaterialTheme.typography.labelSmall,
                                color = palette.textSecondary,
                                maxLines = 1,
                            )
                        }
                    }
                }
                val fallback = message.str("fallback_text")
                val unknown = blocks.any { element ->
                    val block = element as? JsonObject ?: return@any true
                    block.str("type") !in KnownBlockTypes
                }
                if (blocks.isEmpty() && fallback.isNotBlank()) Text(fallback, style = MaterialTheme.typography.bodyLarge)
                if (unknown) Surface(Modifier.fillMaxWidth(), color = MaterialTheme.colorScheme.surface, shape = RoundedCornerShape(8.dp), border = androidx.compose.foundation.BorderStroke(1.dp, palette.borderDefault)) {
                    Text("⚠ ${fallback.ifBlank { stringResource(Res.string.feature_unknown_block) }} · ${stringResource(Res.string.feature_update_client)}", Modifier.padding(10.dp), style = MaterialTheme.typography.bodySmall)
                }
                groupAdjacentQuestions(displayBlocks.mapNotNull { it as? JsonObject }).forEach { blockRun ->
                    val block = blockRun.first()
                    if (block.str("type") == "question" && blockRun.size > 1) {
                            MessageQuestionDeck(
                                messageId = message.str("id"),
                                questionBlocks = blockRun,
                                state = state,
                                onApproval = onApproval,
                                onQuestion = onQuestion,
                                onPreviewFile = onPreviewFile,
                                onExportFile = onExportFile,
                                filePreviews = filePreviews,
                                imagePreviews = imagePreviews,
                                onOpenTrace = onOpenTrace,
                                onOpenProject = onOpenProject,
                                onProjectAction = onProjectAction,
                                onOpenScreen = onOpenScreen,
                                onOpenChat = onOpenChat,
                                onLoopAction = onLoopAction,
                                onTakeover = onTakeover,
                                onReleaseTakeover = onReleaseTakeover,
                                onOpenArtifact = onOpenArtifact,
                                artifactProjectId = artifactProjectId,
                                repository = repository,
                            )
                    } else {
                        BlockView(block, state, onApproval, onQuestion, onPreviewFile, onExportFile, filePreviews, imagePreviews,
                            onOpenTrace, onOpenProject, onProjectAction, onOpenScreen, onOpenChat, onLoopAction, onTakeover, onReleaseTakeover, onOpenArtifact, artifactProjectId, repository)
                    }
                }
                if (assignmentId.isNotBlank()) {
                    Text(stringResource(Res.string.feature_trace_running), Modifier.padding(top = 8.dp), color = MaterialTheme.colorScheme.primary, style = MaterialTheme.typography.labelLarge)
                    Button(onClick = { onOpenTrace(assignmentId, null) }) { Text(stringResource(Res.string.feature_detail)) }
                }
            }
        }
        }
        }
        }
        if (lastOwnMessage) DeliveryView(message.arr("delivery"), state)
        if (showMessageActions) androidx.compose.material3.ModalBottomSheet(onDismissRequest = {
            dismissMessageActions()
        }) {
            Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(stringResource(Res.string.feature_message_actions), style = MaterialTheme.typography.titleMedium)
                if (messageActionError) Text(stringResource(Res.string.chat_delete_failed), color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
                runCatching { Instant.parse(message.str("created_at")).toLocalDateTime(TimeZone.currentSystemDefault()) }
                    .getOrNull()?.let { timestamp ->
                        Text(
                            stringResource(Res.string.chat_time_full, timestamp.year, padChatTimePart(timestamp.month.ordinal + 1), padChatTimePart(timestamp.day), padChatTimePart(timestamp.hour), padChatTimePart(timestamp.minute), padChatTimePart(timestamp.second)),
                            style = MaterialTheme.typography.labelSmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceEvenly) {
                    listOf("👍", "❤️", "😂", "🎉", "🤔", "👀").forEach { emoji ->
                        TextButton(onClick = { if (id.isNotBlank()) onReact(id, emoji); dismissMessageActions() }) { Text(emoji, style = MaterialTheme.typography.titleLarge) }
                    }
                }
                TextButton(onClick = { if (id.isNotBlank()) onQuote(id, "reply"); dismissMessageActions() }) { Text(stringResource(Res.string.chat_menu_reply)) }
                TextButton(onClick = { if (id.isNotBlank()) onQuote(id, "reference"); dismissMessageActions() }) { Text(stringResource(Res.string.chat_menu_quote)) }
                if (actionAssignmentId.isNotBlank()) TextButton(onClick = { onOpenTrace(actionAssignmentId, null); dismissMessageActions() }) {
                    Text(stringResource(Res.string.chat_menu_view_trace))
                }
                val projectId = artifactProjectId.orEmpty()
                val quoteBody = blocks.mapNotNull { entry -> (entry as? JsonObject)?.takeIf { it.str("type") == "text" }?.str("markdown")?.takeIf(String::isNotBlank) }
                    .joinToString("\n\n").ifBlank { message.str("fallback_text") }
                if (projectId.isNotBlank() && quoteBody.isNotBlank()) TextButton(onClick = {
                    val quoted = quoteBody.lines().joinToString("\n") { "> $it" }
                    onQuoteAnnouncement("**$senderLabel**\n$quoted")
                    dismissMessageActions()
                }) { Text(stringResource(Res.string.chat_menu_quote_to_announcement)) }
                TextButton(onClick = { clipboard.setText(AnnotatedString(messageCopyText)); dismissMessageActions() }) { Text(stringResource(Res.string.chat_menu_copy_text)) }
                if (id.isNotBlank() && chatId.isNotBlank()) TextButton(onClick = {
                    clipboard.setText(AnnotatedString(chatMessageDeepLink(hostId, chatId, id)))
                    dismissMessageActions()
                }) { Text(stringResource(Res.string.chat_menu_copy_link)) }
                if (isUser && id.isNotBlank()) TextButton(onClick = {
                    messageActionError = false
                    actionScope.launch {
                        runCatching { repository.callOnHost(hostId, "message.delete", buildJsonObject { put("message_id", id) }) }
                            .onSuccess { dismissMessageActions() }
                            .onFailure { messageActionError = true }
                    }
                }) { Text(stringResource(Res.string.chat_menu_delete), color = MaterialTheme.colorScheme.error) }
            }
        }
    }
}

@Composable
private fun MessageQuestionDeck(
    messageId: String,
    questionBlocks: List<JsonObject>,
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
    val palette = LocalMobilePalette.current
    val previousQuestionLabel = stringResource(Res.string.feature_question_previous)
    val nextQuestionLabel = stringResource(Res.string.feature_question_next)
    val questionIds = questionBlocks.map { it.str("question_id") }
    var selectedQuestion by remember(messageId, questionIds) {
        mutableStateOf(questionBlocks.indexOfFirst { block ->
            state.questions.firstOrNull { it.str("id") == block.str("question_id") }?.str("state") == "pending"
        }.coerceAtLeast(0))
    }
    LaunchedEffect(questionIds, state.questions.map { it.str("id") to it.str("state") }) {
        if (state.questions.firstOrNull { it.str("id") == questionIds.getOrNull(selectedQuestion) }?.str("state") == "answered") {
            val nextPending = questionBlocks.indexOfFirst { block ->
                state.questions.firstOrNull { it.str("id") == block.str("question_id") }?.str("state") == "pending"
            }
            if (nextPending >= 0) selectedQuestion = nextPending
        }
    }
    val answered = questionBlocks.count { block ->
        state.questions.firstOrNull { it.str("id") == block.str("question_id") }?.str("state") == "answered"
    }
    val swipeThreshold = with(androidx.compose.ui.platform.LocalDensity.current) { 48.dp.toPx() }
    Box(
        Modifier.fillMaxWidth().pointerInput(questionIds, swipeThreshold) {
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
                TextButton(enabled = selectedQuestion > 0, onClick = { selectedQuestion-- }, modifier = Modifier.semantics { contentDescription = previousQuestionLabel }) { LucideIcon(LucideIconKind.ChevronLeft, Modifier.size(18.dp)) }
                Row(Modifier.weight(1f).horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.Center, verticalAlignment = Alignment.CenterVertically) {
                    questionBlocks.forEachIndexed { index, block ->
                        val status = state.questions.firstOrNull { it.str("id") == block.str("question_id") }?.str("state")
                        val tabLabel = stringResource(Res.string.block_question_tab, index + 1)
                        TextButton(onClick = { selectedQuestion = index }, modifier = Modifier.semantics { contentDescription = tabLabel }) {
                            Text(if (answered == questionBlocks.size) "✓ $tabLabel" else if (status == "answered") "✓ ${index + 1}" else (index + 1).toString())
                        }
                    }
                }
                TextButton(enabled = selectedQuestion < questionBlocks.lastIndex, onClick = { selectedQuestion++ }, modifier = Modifier.semantics { contentDescription = nextQuestionLabel }) { LucideIcon(LucideIconKind.ChevronRight, Modifier.size(18.dp)) }
            }
            questionBlocks.getOrNull(selectedQuestion)?.let { block ->
                BlockView(block, state, onApproval, onQuestion, onPreviewFile, onExportFile, filePreviews, imagePreviews,
                    onOpenTrace, onOpenProject, onProjectAction, onOpenScreen, onOpenChat, onLoopAction, onTakeover, onReleaseTakeover, onOpenArtifact, artifactProjectId, repository)
            }
        }
    }
}

@Composable
private fun StandaloneTaskCard(
    block: JsonObject,
    state: MobileState,
    onOpenTrace: (String?, String?) -> Unit,
    onLongPress: () -> Unit,
) {
    val palette = LocalMobilePalette.current
    val haptics = LocalHapticFeedback.current
    val assignmentId = block.str("assignment_id")
    val assignment = state.assignments.firstOrNull { it.str("id") == assignmentId }
    val title = assignment?.str("title").orEmpty().ifBlank { stringResource(Res.string.feature_task) }
    val status = assignment?.str("status").orEmpty()
    val botId = assignment?.str("bot_id").orEmpty()
    val botName = state.bots.firstOrNull { it.str("id") == botId }?.str("name").orEmpty().ifBlank { botId }
    val start = assignment?.let {
        runCatching { Instant.parse(it.str("started_at").ifBlank { it.str("created_at") }) }.getOrNull()
    }
    val finish = assignment?.let {
        val finishText = it.str("finished_at").ifBlank { if (status == "done") "" else Clock.System.now().toString() }
        runCatching { Instant.parse(finishText) }.getOrNull()
    }
    val durationMinutes = if (start != null && finish != null) {
        ((finish.toEpochMilliseconds() - start.toEpochMilliseconds()).coerceAtLeast(0L) / 60_000L).coerceAtLeast(1L)
    } else null
    val duration = durationMinutes?.let {
        if (it >= 60) stringResource(Res.string.common_hours_minutes, (it / 60).toInt(), (it % 60).toInt())
        else stringResource(Res.string.common_minutes, it.toInt())
    }
    Surface(
        modifier = Modifier.fillMaxWidth().padding(start = 40.dp, end = 12.dp).combinedClickable(
            onClick = {},
            onLongClick = {
                haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                onLongPress()
            },
        ),
        color = palette.panel,
        shape = RoundedCornerShape(10.dp),
        border = androidx.compose.foundation.BorderStroke(1.dp, if (status == "failed" || status == "blocked") palette.danger else palette.borderSubtle),
    ) {
        Row(Modifier.padding(horizontal = 10.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(if (status == "done") "✓" else if (status in setOf("failed", "blocked")) "!" else "⟳", color = if (status in setOf("failed", "blocked")) palette.danger else palette.success)
            Column(Modifier.weight(1f)) {
                if (status == "done") {
                    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        Text(
                            if (duration.isNullOrBlank()) "$botName · $title" else stringResource(Res.string.block_task_card_done_line, botName, title, duration),
                            modifier = Modifier.weight(1f),
                            style = MaterialTheme.typography.labelSmall,
                            color = palette.textTertiary,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        assignment?.let { AssignmentRecoveryBadge(it) }
                    }
                } else {
                    Text(
                        stringResource(Res.string.block_task_card_title, title),
                        style = MaterialTheme.typography.labelMedium,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
                        if (status.isNotBlank()) StatusLabel(status)
                        assignment?.let { AssignmentRecoveryBadge(it) }
                        duration?.let { Text(stringResource(Res.string.block_task_card_elapsed, it), style = MaterialTheme.typography.labelSmall, color = palette.textTertiary) }
                    }
                }
            }
            if (assignmentId.isNotBlank()) TextButton(onClick = { onOpenTrace(assignmentId, null) }, contentPadding = PaddingValues(horizontal = 6.dp, vertical = 0.dp)) {
                Text(stringResource(Res.string.feature_detail), style = MaterialTheme.typography.labelSmall)
            }
        }
    }
}

private val KnownBlockTypes = setOf(
    "text", "image", "file", "task_card", "completion", "progress", "blocked", "question", "project_card", "review_card",
    "delegation", "approval", "approval_ref", "takeover_request", "bot_dm_ref", "system", "loop_paused", "link_preview",
)

@Composable
private fun SlashSkillOption(skill: JsonObject, onSelect: (String) -> Unit) {
    val name = skill.str("name")
    TextButton(
        onClick = { onSelect(name) },
        modifier = Modifier.fillMaxWidth().heightIn(min = 60.dp),
    ) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            Surface(color = LocalMobilePalette.current.accentSoft, shape = RoundedCornerShape(8.dp), modifier = Modifier.size(36.dp)) {
                Box(contentAlignment = Alignment.Center) {
                    LucideIcon(LucideIconKind.Zap, Modifier.size(18.dp), tint = LocalMobilePalette.current.accent)
                }
            }
            Column(Modifier.weight(1f), horizontalAlignment = Alignment.Start, verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text("/$name", style = MaterialTheme.typography.titleSmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(skill.str("description"), style = MaterialTheme.typography.bodySmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
    }
}

@Composable
private fun MessageSkillChip(name: String) {
    val palette = LocalMobilePalette.current
    Surface(
        color = palette.accentSoft,
        contentColor = palette.accent,
        shape = RoundedCornerShape(8.dp),
        modifier = Modifier.widthIn(max = 220.dp),
    ) {
        Row(
            Modifier.padding(horizontal = 8.dp, vertical = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            LucideIcon(LucideIconKind.Zap, Modifier.size(14.dp))
            Text(name, style = MaterialTheme.typography.labelMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}

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
            val size = file.long("size")
            Surface(
                modifier = Modifier.fillMaxWidth(),
                shape = RoundedCornerShape(12.dp),
                color = LocalMobilePalette.current.panel,
                border = androidx.compose.foundation.BorderStroke(1.dp, LocalMobilePalette.current.borderSubtle),
            ) {
                Row(
                    modifier = Modifier.padding(10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    Box(
                        modifier = Modifier
                            .size(40.dp)
                            .clip(RoundedCornerShape(10.dp))
                            .background(LocalMobilePalette.current.accentSoft),
                        contentAlignment = Alignment.Center,
                    ) {
                        LucideIcon(LucideIconKind.FileText, Modifier.size(20.dp), LocalMobilePalette.current.accent)
                    }
                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                        Text(
                            file.str("name").ifBlank { file.str("path") },
                            style = MaterialTheme.typography.bodyMedium,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        Text(fileSizeLabel(size), style = MaterialTheme.typography.bodySmall, color = LocalMobilePalette.current.textSecondary)
                    }
                    LucideIcon(LucideIconKind.ChevronDown, Modifier.size(16.dp), LocalMobilePalette.current.textTertiary)
                }
            }
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                TextButton(onClick = { onPreviewFile(file) }) { Text(stringResource(Res.string.feature_preview_file)) }
                TextButton(onClick = { onExportFile(file) }) { Text(stringResource(Res.string.feature_export_file)) }
            }
            filePreviews[key]?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
        }
        "task_card" -> {
            val assignmentId = block.str("assignment_id")
            val assignment = state.assignments.firstOrNull { it.str("id") == assignmentId }
            val assignmentTitle = assignment?.str("title").orEmpty()
            val assignmentLabel = if (assignmentTitle.isBlank()) stringResource(Res.string.feature_task) else assignmentTitle
            if (assignment?.str("status") == "done") {
                val start = runCatching { Instant.parse(assignment.str("started_at").ifBlank { assignment.str("created_at") }) }.getOrNull()
                val finish = runCatching { Instant.parse(assignment.str("finished_at")) }.getOrNull()
                val elapsedMinutes = if (start != null && finish != null) {
                    ((finish.toEpochMilliseconds() - start.toEpochMilliseconds()).coerceAtLeast(0L) / 60_000L).coerceAtLeast(1L)
                } else null
                val duration = elapsedMinutes?.let {
                    if (it >= 60) stringResource(Res.string.common_hours_minutes, (it / 60).toInt(), (it % 60).toInt())
                    else stringResource(Res.string.common_minutes, it.toInt())
                }.orEmpty()
                val botId = assignment.str("bot_id")
                val botName = state.bots.firstOrNull { it.str("id") == botId }?.str("name").orEmpty().ifBlank { botId }
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text("✓", color = LocalMobilePalette.current.success, style = MaterialTheme.typography.labelMedium)
                    Text(
                        if (duration.isBlank()) "$botName · $assignmentLabel" else stringResource(Res.string.block_task_card_done_line, botName, assignmentLabel, duration),
                        Modifier.weight(1f),
                        style = MaterialTheme.typography.labelSmall,
                        color = LocalMobilePalette.current.textTertiary,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    assignment?.let { AssignmentRecoveryBadge(it) }
                    if (assignmentId.isNotBlank()) TextButton(onClick = { onOpenTrace(assignmentId, null) }, contentPadding = PaddingValues(horizontal = 6.dp, vertical = 0.dp)) {
                        Text(stringResource(Res.string.feature_detail), style = MaterialTheme.typography.labelSmall)
                    }
                }
            } else {
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                    Text(stringResource(Res.string.block_task_card_title, assignmentLabel), Modifier.weight(1f), style = MaterialTheme.typography.titleSmall)
                    assignment?.str("status")?.takeIf { it.isNotBlank() }?.let { StatusLabel(it) }
                    assignment?.let { AssignmentRecoveryBadge(it) }
                }
                if (assignmentId.isNotBlank()) TextButton(onClick = { onOpenTrace(assignmentId, null) }) { Text(stringResource(Res.string.feature_detail)) }
            }
        }
        "completion" -> {
            Surface(
                modifier = Modifier.fillMaxWidth(),
                color = LocalMobilePalette.current.panel,
                shape = RoundedCornerShape(10.dp),
                border = androidx.compose.foundation.BorderStroke(1.dp, LocalMobilePalette.current.borderDefault),
            ) {
                Column(Modifier.padding(horizontal = 10.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        Text("✓", color = LocalMobilePalette.current.success, style = MaterialTheme.typography.titleSmall)
                        Text(stringResource(Res.string.block_completion_done), style = MaterialTheme.typography.labelMedium, color = LocalMobilePalette.current.success)
                    }
                    MarkdownText(block.str("summary"), Modifier.fillMaxWidth())
                    block.arr("artifacts").forEach { element ->
                        val artifact = element as? JsonObject ?: return@forEach
                        val url = artifact.str("path_or_url")
                        val author = state.bots.firstOrNull { it.str("id") == artifact.str("bot_id") }?.str("name").orEmpty().ifBlank { artifact.str("bot_id") }
                        Surface(
                            modifier = Modifier.fillMaxWidth(),
                            color = LocalMobilePalette.current.bubbleIn,
                            shape = RoundedCornerShape(8.dp),
                            onClick = { onOpenArtifact(artifact.str("artifact_id"), url, artifactProjectId) },
                        ) {
                            Row(Modifier.padding(8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                                Surface(Modifier.size(30.dp), color = MaterialTheme.colorScheme.primaryContainer, shape = RoundedCornerShape(6.dp)) {
                                    Box(contentAlignment = Alignment.Center) { LucideIcon(LucideIconKind.FileText, Modifier.size(18.dp)) }
                                }
                                Column(Modifier.weight(1f)) {
                                    Text(artifact.str("title").ifBlank { url }, style = MaterialTheme.typography.bodySmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                    Text(stringResource(Res.string.feature_artifact_author, author), style = MaterialTheme.typography.labelSmall, color = LocalMobilePalette.current.textTertiary, maxLines = 1)
                                    Text(url, style = MaterialTheme.typography.labelSmall, color = LocalMobilePalette.current.textTertiary, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                }
                                Text(stringResource(Res.string.feature_preview_file), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.primary)
                            }
                        }
                    }
                    block.arr("next").forEach { element ->
                        val handoff = element as? JsonObject ?: return@forEach
                        val botId = handoff.str("bot_id")
                        val botName = state.bots.firstOrNull { it.str("id") == botId }?.str("name").orEmpty().ifBlank { botId }
                        val targetChat = state.chats.firstOrNull { it.str("bot_id") == botId && it.str("kind") in listOf("direct", "bot_dm") }?.str("id")
                        val nextInstruction = stringResource(Res.string.feature_completion_next, botName)
                        if (targetChat != null) TextButton(onClick = { onOpenChat(targetChat) }, contentPadding = PaddingValues(horizontal = 6.dp, vertical = 2.dp)) {
                            Text("› $nextInstruction · ${handoff.str("instruction")}", style = MaterialTheme.typography.bodySmall, maxLines = 2, overflow = TextOverflow.Ellipsis)
                        } else Text("› $nextInstruction · ${handoff.str("instruction")}", style = MaterialTheme.typography.bodySmall)
                    }
                    if (block.boolean("notify_main")) Text("› ${stringResource(Res.string.block_completion_notify_main)}", style = MaterialTheme.typography.labelMedium)
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
                stringResource(if (question?.boolean("multi") == true) Res.string.block_question_multi else Res.string.block_question_single),
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.primary,
            )
            Text(questionText, style = MaterialTheme.typography.titleSmall)
            when {
                question?.str("state") == "expired" -> {
                    question.arr("options").forEach { option ->
                        Text(option.toString().trim('"'), color = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = .4f))
                    }
                    Text(stringResource(Res.string.feature_decision_expired), Modifier.semantics { liveRegion = LiveRegionMode.Polite }, color = MaterialTheme.colorScheme.onSurfaceVariant)
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
                        modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite },
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
                                Text(stringResource(Res.string.block_question_your_reply), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.primary)
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
                                Text(stringResource(Res.string.block_question_other))
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
            Surface(
                modifier = Modifier.fillMaxWidth(),
                color = LocalMobilePalette.current.panel,
                shape = RoundedCornerShape(10.dp),
                border = androidx.compose.foundation.BorderStroke(1.dp, LocalMobilePalette.current.borderDefault),
            ) {
                Column(Modifier.padding(10.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        LucideIcon(LucideIconKind.Target, Modifier.size(16.dp), tint = MaterialTheme.colorScheme.tertiary)
                        Text(stringResource(Res.string.block_review_card_title, reviewProjectLabel(block, state.projects)), Modifier.weight(1f), style = MaterialTheme.typography.labelLarge)
                        if (reviewState != "pending") StatusLabel(reviewState)
                    }
                    when (reviewState) {
                        "confirmed" -> Text(stringResource(Res.string.feature_review_confirmed), Modifier.semantics { liveRegion = LiveRegionMode.Polite }, color = LocalMobilePalette.current.success, style = MaterialTheme.typography.labelMedium)
                        "changes_requested" -> Text(stringResource(Res.string.feature_review_changes_requested), Modifier.semantics { liveRegion = LiveRegionMode.Polite }, color = MaterialTheme.colorScheme.secondary, style = MaterialTheme.typography.labelMedium)
                    }
                    block.arr("artifacts").forEach { element ->
                        val artifact = element as? JsonObject ?: return@forEach
                        val artifactId = artifact.str("artifact_id")
                        val pathOrUrl = artifact.str("path_or_url")
                        if (artifactId.isNotBlank() && pathOrUrl.isNotBlank()) {
                            Surface(
                                modifier = Modifier.fillMaxWidth(),
                                color = LocalMobilePalette.current.bubbleIn,
                                shape = RoundedCornerShape(8.dp),
                                onClick = { onOpenArtifact(artifactId, pathOrUrl, projectId.takeIf { it.isNotBlank() }) },
                            ) {
                                Row(Modifier.padding(8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                                    Surface(Modifier.size(28.dp), color = MaterialTheme.colorScheme.primaryContainer, shape = RoundedCornerShape(6.dp)) {
                                        Box(contentAlignment = Alignment.Center) { LucideIcon(LucideIconKind.FileText, Modifier.size(16.dp)) }
                                    }
                                    Column(Modifier.weight(1f)) {
                                        Text(artifact.str("title").ifBlank { pathOrUrl }, style = MaterialTheme.typography.bodySmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                        val authorId = artifact.str("bot_id")
                                        val author = state.bots.firstOrNull { it.str("id") == authorId }?.str("name").orEmpty().ifBlank { authorId }
                                        if (author.isNotBlank()) Text(stringResource(Res.string.feature_artifact_author, author), style = MaterialTheme.typography.labelSmall, color = LocalMobilePalette.current.textTertiary)
                                        Text(pathOrUrl, style = MaterialTheme.typography.labelSmall, color = LocalMobilePalette.current.textTertiary, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                    }
                                    Text(stringResource(Res.string.feature_preview_file), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.primary)
                                }
                            }
                        }
                    }
                    if (reviewState == "pending" && projectId.isNotBlank()) {
                        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            Button(onClick = { onProjectAction(projectId, "confirm_done") }, modifier = Modifier.weight(1f).heightIn(min = 48.dp), contentPadding = PaddingValues(horizontal = 8.dp, vertical = 4.dp)) {
                                Text(stringResource(Res.string.block_review_card_confirm), maxLines = 1, overflow = TextOverflow.Ellipsis)
                            }
                            OutlinedButton(onClick = { onProjectAction(projectId, "request_changes") }, modifier = Modifier.weight(1f).heightIn(min = 48.dp), contentPadding = PaddingValues(horizontal = 8.dp, vertical = 4.dp)) {
                                Text(stringResource(Res.string.block_review_card_request_changes), maxLines = 1, overflow = TextOverflow.Ellipsis)
                            }
                        }
                    }
                    if (projectId.isNotBlank()) TextButton(onClick = { onOpenProject(projectId) }, contentPadding = PaddingValues(horizontal = 8.dp, vertical = 2.dp)) {
                        Text(stringResource(Res.string.feature_review_enter_group))
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
            val alwaysAllowSupported = approval?.let { ApprovalWireModel(it).alwaysAllowSupported } ?: true
            val matchedRuleText = approval?.let { ApprovalWireModel(it).matchedRuleText }?.takeIf(String::isNotBlank)
            Text(stringResource(Res.string.feature_approval_needed), Modifier.semantics { liveRegion = LiveRegionMode.Assertive }, style = MaterialTheme.typography.titleSmall)
            Text(approval?.str("summary").orEmpty().ifBlank { approvalId })
            MarkdownText(approval?.str("detail").orEmpty(), Modifier.fillMaxWidth())
            matchedRuleText?.let { ruleText ->
                val assignmentId = approval?.str("assignment_id").orEmpty()
                val projectId = state.assignments.firstOrNull { it.str("id") == assignmentId }?.str("project_id").orEmpty()
                val sourceName = state.projects.firstOrNull { it.str("id") == projectId }?.str("name")?.takeIf(String::isNotBlank)
                    ?: state.bots.firstOrNull { it.str("id") == approval?.str("bot_id") }?.str("name")?.takeIf(String::isNotBlank)
                    ?: stringResource(Res.string.feature_bot)
                Text(
                    stringResource(Res.string.block_approval_meta, sourceName, ruleText),
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            if (approval?.str("state") == "pending") Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Button(onClick = { onApproval(approvalId, "allow_once") }) { Text(stringResource(Res.string.block_approval_allow_once)) }
                if (alwaysAllowSupported) Button(onClick = { onApproval(approvalId, "always_allow") }) { Text(stringResource(Res.string.block_approval_always_allow)) }
                Button(onClick = { onApproval(approvalId, "deny") }) { Text(stringResource(Res.string.block_approval_deny)) }
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
                    Text(stringResource(Res.string.feature_takeover_pending), Modifier.semantics { liveRegion = LiveRegionMode.Assertive }, style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.tertiary)
                    if (botId.isNotBlank()) TakeoverFramePreview(repository, botId)
                    if (botId.isNotBlank()) Button(onClick = { onTakeover(botId) }, modifier = Modifier.fillMaxWidth().heightIn(min = 44.dp)) {
                        Text(stringResource(Res.string.feature_takeover))
                    }
                }
                "active" -> {
                    Text(stringResource(Res.string.feature_takeover_active), Modifier.semantics { liveRegion = LiveRegionMode.Polite }, style = MaterialTheme.typography.labelMedium)
                    if (botId.isNotBlank()) {
                        Button(onClick = { onOpenScreen(botId, null) }, modifier = Modifier.fillMaxWidth().heightIn(min = 44.dp)) {
                            Text(stringResource(Res.string.feature_open_screen))
                        }
                        TextButton(onClick = { onReleaseTakeover(botId) }, modifier = Modifier.fillMaxWidth().heightIn(min = 44.dp)) {
                            Text(stringResource(Res.string.feature_return_control))
                        }
                    }
                }
                "done" -> Text(stringResource(Res.string.feature_takeover_done), Modifier.semantics { liveRegion = LiveRegionMode.Polite }, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
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

@Composable
private fun TakeoverFramePreview(repository: MobileRepository, botId: String) {
    var image by remember(botId) { mutableStateOf<ImageBitmap?>(null) }
    var site by remember(botId) { mutableStateOf("") }
    val decoder = remember { platformScreenImageDecoder() }
    val noFrame = stringResource(Res.string.computer_no_frame)
    val previewLabel = stringResource(Res.string.feature_takeover_preview)
    LaunchedEffect(repository, botId) {
        val connection = runCatching {
            repository.createScreen(botId, "low", null) { incoming ->
                val decoded = decoder.decodeJpeg(incoming.jpeg) ?: return@createScreen
                withContext(Dispatchers.Main.immediate) {
                    image = decoded
                    site = incoming.header.url
                }
            }
        }.getOrNull() ?: return@LaunchedEffect
        try {
            connection.start().join()
        } finally {
            connection.stop()
        }
    }
    androidx.compose.foundation.layout.Box(
        Modifier.fillMaxWidth().height(72.dp)
            .clip(RoundedCornerShape(8.dp))
            .background(MaterialTheme.colorScheme.surfaceVariant)
            .semantics {
                contentDescription = if (image == null) "$previewLabel, $noFrame" else "$previewLabel, ${site.ifBlank { noFrame }}"
            },
        contentAlignment = Alignment.Center,
    ) {
        val currentImage = image
        if (currentImage == null) Text(noFrame, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        else Image(currentImage, contentDescription = null, modifier = Modifier.fillMaxSize(), contentScale = ContentScale.Crop)
    }
}

internal fun fileRef(block: JsonObject): JsonObject {
    val file = block.obj("file")
    return if (file.str("path").isNotBlank() || file.str("root").isNotBlank()) file else block
}

internal fun fileKey(file: JsonObject): String = "${file.str("root")}:${file.str("root_id")}:${file.str("path")}"

@Composable
private fun fileSizeLabel(sizeBytes: Long): String {
    val size = sizeBytes.coerceAtLeast(0L)
    return when {
        size < 1_024L -> stringResource(Res.string.feature_file_size_bytes, size.toInt())
        size < 1_048_576L -> stringResource(Res.string.feature_file_size_kb, ((size + 512L) / 1_024L).toInt())
        size < 1_073_741_824L -> {
            val tenths = (size * 10L / 1_048_576L).toInt()
            stringResource(Res.string.feature_file_size_mb, tenths / 10, tenths % 10)
        }
        else -> {
            val tenths = ((size / 1_073_741_824L) * 10L + (size % 1_073_741_824L) * 10L / 1_073_741_824L)
                .coerceAtMost(Int.MAX_VALUE.toLong()).toInt()
            stringResource(Res.string.feature_file_size_gb, tenths / 10, tenths % 10)
        }
    }
}
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
