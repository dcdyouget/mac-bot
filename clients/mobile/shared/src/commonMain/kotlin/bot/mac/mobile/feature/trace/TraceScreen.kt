package bot.mac.mobile.feature.trace

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.protocol.boolean
import bot.mac.mobile.core.protocol.arr
import bot.mac.mobile.core.protocol.long
import bot.mac.mobile.core.protocol.objects
import bot.mac.mobile.core.protocol.obj
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.design.FontScaleToolbar
import bot.mac.mobile.core.network.NetworkError
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.core.ui.MarkdownText
import bot.mac.mobile.core.ui.LucideIcon
import bot.mac.mobile.core.ui.LucideIconKind
import bot.mac.mobile.feature.chat.StatusLabel
import bot.mac.mobile.resources.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.jetbrains.compose.resources.stringResource

@Composable
fun TraceScreen(
    repository: MobileRepository,
    assignmentId: String? = null,
    chatId: String? = null,
    onOpenScreen: (botId: String, tabId: String?) -> Unit = { _, _ -> },
    onSteer: (assignmentId: String, text: String) -> Unit = { _, _ -> },
    onOpenAssignmentTrace: (String) -> Unit = {},
    onBack: () -> Unit = {},
) {
    val state by repository.state.collectAsState()
    val streamKey = assignmentId ?: chatId ?: ""
    val items = state.traces[streamKey].orEmpty()
    var filter by remember { mutableStateOf<TraceFilter?>(null) }
    var live by remember { mutableStateOf(false) }
    var stream by remember { mutableStateOf<String?>(null) }
    var firstAseq by remember { mutableStateOf<Long?>(null) }
    var hasMoreBefore by remember { mutableStateOf(false) }
    var search by remember { mutableStateOf("") }
    var showThinking by remember { mutableStateOf(false) }
    var showOutput by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var fullOutputs by remember { mutableStateOf(emptyMap<String, String>()) }
    var collapsedRuns by remember { mutableStateOf(emptySet<String>()) }
    var steerText by remember { mutableStateOf("") }
    var retryingAssignment by remember(assignmentId) { mutableStateOf(false) }
    var queuedLoading by remember { mutableStateOf(false) }
    var queuedError by remember { mutableStateOf<String?>(null) }
    var steering by remember { mutableStateOf<Set<String>>(emptySet()) }
    var staleSteers by remember { mutableStateOf<Set<String>>(emptySet()) }
    val fragments = state.traceFragments.filterKeys { it.startsWith("${stream ?: streamKey}:") }
    val scope = rememberCoroutineScope()
    val activeFilter = filter
    val traceError = stringResource(Res.string.feature_trace_error)
    val currentAssignment = assignmentId?.let { id -> state.assignments.firstOrNull { it.str("id") == id } }
    val currentBotId = currentAssignment?.str("bot_id").orEmpty()
    val currentBot = state.bots.firstOrNull { it.str("id") == currentBotId }
    val currentBotName = state.bots.firstOrNull { it.str("id") == currentBotId }?.str("name").orEmpty().ifBlank { currentBotId }
    val currentBotLabel = currentBotName.ifBlank { stringResource(Res.string.search_filter_bots) }
    val currentTitle = currentAssignment?.str("title").orEmpty().ifBlank { stringResource(Res.string.feature_task) }
    val assignmentProjectId = currentAssignment?.str("project_id").orEmpty()
    val assignmentChat = state.chats.firstOrNull { it.str("project_id") == assignmentProjectId && assignmentProjectId.isNotBlank() }
    val assignmentChatId = assignmentChat?.str("id").orEmpty()
    val queuedMessages = state.messages[assignmentChatId].orEmpty()
        .filter { it.str("assignment_id") == assignmentId }
        .flatMap { message ->
            message.arr("delivery").mapNotNull { it as? JsonObject }
                .filter { it.str("state") == "queued" && it.str("bot_id").isNotBlank() }
                .map { delivery ->
                    val key = "${message.str("id")}:${delivery.str("bot_id")}:$assignmentId"
                    val lastRead = assignmentChat?.long("last_read_seq") ?: Long.MAX_VALUE
                    QueuedSteer(message, delivery, key, message.long("seq") <= lastRead)
                }
        }
        .distinctBy { it.key }
        .sortedBy { it.message.long("seq") }

    // chat.history populates the repository cache used below; chat.list handles a
    // trace opened before the chat list has arrived in the initial snapshot.
    LaunchedEffect(assignmentId, state.connected, assignmentChatId, assignmentProjectId) {
        if (assignmentId.isNullOrBlank() || !state.connected) return@LaunchedEffect
        queuedLoading = true
        queuedError = null
        try {
            if (assignmentChatId.isBlank()) {
                if (assignmentProjectId.isNotBlank()) repository.call("chat.list", buildJsonObject {})
            } else {
                repository.call("chat.history", buildJsonObject {
                    put("chat_id", assignmentChatId)
                    put("limit", 100)
                })
            }
        } catch (failure: Throwable) {
            queuedError = failure.message
        } finally {
            queuedLoading = false
        }
    }

    LaunchedEffect(streamKey, state.connected) {
        if (streamKey.isBlank() || !state.connected) return@LaunchedEffect
        var localStream: String? = null
        try {
            val result = repository.call("trace.history", buildJsonObject {
                if (assignmentId != null) put("assignment_id", assignmentId) else put("chat_id", chatId)
                put("tail", true)
                put("limit", 200)
            })
            firstAseq = result.longValue("first_aseq")
            hasMoreBefore = result.boolean("has_more_before")
            val last = result.longValue("last_aseq") ?: items.maxOfOrNull { it.longValue("aseq") ?: 0L } ?: 0L
            live = result.boolean("live")
            if (live) {
                val sub = repository.call("trace.subscribe", buildJsonObject {
                    if (assignmentId != null) put("assignment_id", assignmentId) else put("chat_id", chatId)
                    put("since_aseq", last)
                })
                localStream = sub.str("stream").takeIf { it.isNotBlank() }
                stream = localStream
                awaitCancellation()
            }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (failure: Throwable) {
            error = failure.message ?: traceError
        } finally {
            localStream?.let { streamId ->
                if (repository.state.value.connected) {
                    withContext(NonCancellable) {
                        runCatching {
                            withTimeout(2_000) { repository.call("trace.unsubscribe", buildJsonObject { put("stream", streamId) }) }
                        }
                    }
                }
            }
            if (stream == localStream) stream = null
        }
    }

    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        FontScaleToolbar {
            Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp)) {
                Button(onClick = onBack) { Text(stringResource(Res.string.common_back)) }
                Column(Modifier.weight(1f).padding(start = 12.dp)) {
                    Text(stringResource(Res.string.feature_trace), style = MaterialTheme.typography.titleLarge)
                    if (assignmentId != null) {
                    Text(
                        stringResource(Res.string.feature_trace_assignment_context, currentTitle, currentBotLabel),
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        style = MaterialTheme.typography.labelSmall,
                    )
                    Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        currentAssignment?.str("status")?.takeIf { it.isNotBlank() }?.let { StatusLabel(it) }
                        if (live) Text(stringResource(Res.string.feature_realtime), color = MaterialTheme.colorScheme.primary, style = MaterialTheme.typography.labelSmall)
                    }
                    if (currentBotId.isNotBlank() && (currentBot?.obj("tools")?.boolean("browser") == true || currentBot?.str("browser_mode").orEmpty().isNotBlank())) {
                        Button(onClick = { onOpenScreen(currentBotId, null) }) { Text(stringResource(Res.string.feature_open_screen)) }
                    }
                    } else if (live) {
                        Text(stringResource(Res.string.feature_realtime), color = MaterialTheme.colorScheme.primary, style = MaterialTheme.typography.labelSmall)
                    }
                }
            }
        }
        if (assignmentId != null) {
            Column(
                Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp)
                    .background(MaterialTheme.colorScheme.secondaryContainer)
                    .padding(12.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Text(
                    "${stringResource(Res.string.status_queued)} · ${stringResource(Res.string.feature_trace_steer_hint)}",
                    style = MaterialTheme.typography.titleSmall,
                )
                if (!state.connected) {
                    Text(stringResource(Res.string.followup_offline), color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
                }
                queuedError?.takeIf { it.isNotBlank() }?.let {
                    Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
                }
                if (queuedLoading && queuedMessages.isEmpty()) {
                    Text(stringResource(Res.string.feature_history_loading), style = MaterialTheme.typography.bodySmall)
                }
                queuedMessages.forEach { queued ->
                    val botName = state.bots.firstOrNull { it.str("id") == queued.delivery.str("bot_id") }
                        ?.str("name").orEmpty().ifBlank { queued.delivery.str("bot_id") }
                    Column(Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surface).padding(10.dp)) {
                        Text(botName, style = MaterialTheme.typography.labelLarge)
                        Text(queued.message.str("fallback_text").ifBlank { stringResource(Res.string.feature_task) }, maxLines = 3, overflow = TextOverflow.Ellipsis)
                        if (queued.read || queued.key in staleSteers) {
                            Text(stringResource(Res.string.feature_trace_steer_stale), color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodySmall)
                        } else {
                            Button(
                                enabled = state.connected && queued.key !in steering,
                                onClick = {
                                    steering = steering + queued.key
                                    scope.launch {
                                        runCatching {
                                            repository.call("message.steer", buildJsonObject {
                                                put("message_id", queued.message.str("id"))
                                                put("bot_id", queued.delivery.str("bot_id"))
                                                put("assignment_id", assignmentId)
                                            })
                                        }.onFailure { failure ->
                                            if ((failure as? NetworkError)?.code == "conflict") staleSteers = staleSteers + queued.key
                                            else queuedError = failure.message ?: ""
                                        }
                                        steering = steering - queued.key
                                    }
                                },
                            ) {
                                Text(if (queued.key in steering) stringResource(Res.string.feature_loading) else stringResource(Res.string.trace_queue_to_steer))
                            }
                        }
                    }
                }
            }
        }
        Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            TraceFilter.entries.forEach { candidate ->
                FilterChip(selected = filter == candidate, onClick = { filter = if (filter == candidate) null else candidate }, label = { Text(candidate.label()) })
            }
        }
        Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlinedTextField(search, { search = it }, Modifier.weight(1f), singleLine = true, placeholder = { Text(stringResource(Res.string.trace_search)) })
            Column {
                Row { Text(stringResource(Res.string.feature_trace_thinking)); Switch(showThinking, { showThinking = it }) }
                Row { Text(stringResource(Res.string.feature_trace_output)); Switch(showOutput, { showOutput = it }) }
            }
        }
        error?.let { Text(it, Modifier.padding(horizontal = 12.dp), color = MaterialTheme.colorScheme.error) }
        HorizontalDivider(Modifier.padding(top = 8.dp))
        LazyColumn(Modifier.weight(1f).fillMaxWidth().padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            if (hasMoreBefore && firstAseq != null) item {
                Button(onClick = {
                    scope.launch {
                        runCatching { repository.call("trace.history", buildJsonObject { if (assignmentId != null) put("assignment_id", assignmentId) else put("chat_id", chatId); put("before_aseq", firstAseq); put("limit", 200) }) }
                            .onSuccess { result ->
                                firstAseq = result.longValue("first_aseq") ?: result.objects("items").minOfOrNull { it.longValue("aseq") ?: Long.MAX_VALUE }
                                hasMoreBefore = result.boolean("has_more_before")
                            }
                            .onFailure { error = it.message }
                    }
                }) { Text(stringResource(Res.string.feature_trace_older)) }
            }
            items(items.filter { item ->
                val parent = item.obj("data").str("parent_run_id")
                parent.isBlank() || parent !in collapsedRuns
            }.filter { (activeFilter == null || activeFilter.accept(it)) && (search.isBlank() || it.toString().contains(search, ignoreCase = true)) }, key = { "${it.str("run_id")}:${it.longValue("aseq")}:${it.hashCode()}" }) { item ->
                TraceItemCard(item, fragments, onOpenScreen, showThinking, showOutput, fullOutputs,
                    runCollapsed = item.str("run_id") in collapsedRuns,
                    onToggleRun = { runId -> collapsedRuns = if (runId in collapsedRuns) collapsedRuns - runId else collapsedRuns + runId },
                    onFetchFull = { runId, callId ->
                        scope.launch {
                            val key = "$runId:$callId"
                            runCatching { repository.fetchText("/api/v1/trace/output", mapOf("run_id" to runId, "call_id" to callId)) }
                                .onSuccess { fullOutputs = fullOutputs + (key to it) }
                                .onFailure { error = it.message }
                        }
                    })
            }
            if (items.isEmpty() && fragments.isEmpty()) item { Text(stringResource(Res.string.feature_no_trace), Modifier.padding(24.dp)) }
        }
        if (assignmentId != null) {
            val assignmentStatus = state.assignments.firstOrNull { it.str("id") == assignmentId }?.str("status")
            val canStop = assignmentStatus in setOf("queued", "working", "waiting_user", "waiting_bot", "blocked")
            Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(
                    value = steerText,
                    onValueChange = { steerText = it },
                    modifier = Modifier.weight(1f),
                    singleLine = true,
                    enabled = state.connected && canStop,
                    label = { Text(stringResource(Res.string.feature_trace_steer_hint)) },
                )
                Button(
                    enabled = state.connected && canStop && steerText.isNotBlank(),
                    onClick = { onSteer(assignmentId, steerText.trim()); steerText = "" },
                ) { Text(stringResource(Res.string.feature_trace_steer_action)) }
                Button(enabled = canStop, onClick = { scope.launch { runCatching { repository.call("assignment.stop", buildJsonObject { put("assignment_id", assignmentId) }) }.onFailure { error = it.message } } }) { Text(stringResource(Res.string.common_stop)) }
                if (assignmentStatus == "failed") Button(
                    enabled = state.connected && !retryingAssignment,
                    onClick = {
                        retryingAssignment = true
                        scope.launch {
                            runCatching { repository.call("assignment.retry", buildJsonObject { put("assignment_id", assignmentId) }) }
                                .onSuccess { result -> result.obj("assignment").str("id").takeIf(String::isNotBlank)?.let(onOpenAssignmentTrace) }
                                .onFailure { error = it.message }
                            retryingAssignment = false
                        }
                    },
                ) { Text(stringResource(if (retryingAssignment) Res.string.feature_assignment_retrying else Res.string.feature_assignment_retry)) }
            }
        }
    }
}

private data class QueuedSteer(
    val message: JsonObject,
    val delivery: JsonObject,
    val key: String,
    val read: Boolean,
)

private enum class TraceFilter {
    ALL, MODEL, TOOL, MESSAGE, WAIT;
    @Composable fun label(): String = when (this) {
        ALL -> stringResource(Res.string.common_all)
        MODEL -> stringResource(Res.string.feature_trace_model)
        TOOL -> stringResource(Res.string.feature_trace_tool)
        MESSAGE -> stringResource(Res.string.feature_trace_message)
        WAIT -> stringResource(Res.string.feature_trace_wait)
    }
    fun accept(item: JsonObject): Boolean = when (this) {
        ALL -> true
        MODEL -> item.str("type")?.startsWith("llm.") == true
        TOOL -> item.str("type")?.startsWith("tool.") == true
        MESSAGE -> item.str("type") == "send_msg" || item.str("type") == "steer"
        WAIT -> item.str("type") == "run.wait" || item.str("type") == "run.resume"
    }
}

@Composable
private fun TraceItemCard(
    item: JsonObject,
    fragments: Map<String, String>,
    onOpenScreen: (String, String?) -> Unit,
    showThinking: Boolean,
    showOutput: Boolean,
    fullOutputs: Map<String, String>,
    runCollapsed: Boolean,
    onToggleRun: (String) -> Unit,
    onFetchFull: (runId: String, callId: String) -> Unit,
) {
    val type = item.str("type").ifBlank { "event" }
    val data = item.obj("data")
    val runDisclosureLabel = stringResource(if (runCollapsed) Res.string.feature_trace_run_expand else Res.string.feature_trace_run_collapse)
    val summary = when (type) {
        "run.start" -> stringResource(Res.string.feature_trace_start, data?.str("phase") ?: "")
        "run.end" -> stringResource(Res.string.feature_trace_end_label)
        "llm.request" -> stringResource(Res.string.feature_trace_request, data?.str("model") ?: "")
        "llm.response" -> stringResource(Res.string.feature_trace_response, data?.str("stop_reason") ?: "")
        "tool.start" -> stringResource(Res.string.feature_trace_tool_start, data?.str("name") ?: "")
        "tool.end" -> stringResource(Res.string.feature_trace_tool_end, data?.str("preview") ?: "")
        "send_msg" -> stringResource(Res.string.feature_trace_send, data?.str("intent") ?: "")
        "steer" -> stringResource(Res.string.feature_trace_steer, data?.str("text") ?: "")
        "run.wait" -> stringResource(Res.string.feature_trace_waiting, data?.str("reason") ?: "")
        "run.resume" -> stringResource(Res.string.feature_trace_resume)
        else -> type
    }
    val parentRunId = data.str("parent_run_id")
    Column(Modifier.fillMaxWidth().padding(start = if (parentRunId.isBlank()) 0.dp else 20.dp).background(MaterialTheme.colorScheme.surfaceVariant).padding(12.dp)) {
        Row(Modifier.fillMaxWidth()) {
            Column(Modifier.weight(1f)) {
                Text(summary, style = MaterialTheme.typography.titleSmall)
                if (type == "run.end") data.str("status").takeIf { it.isNotBlank() }?.let { StatusLabel(it) }
            }
            Text(item.str("at"), style = MaterialTheme.typography.labelSmall)
            if (type == "run.start" && item.str("run_id").isNotBlank()) {
                Button(onClick = { onToggleRun(item.str("run_id")) }, modifier = Modifier.semantics { contentDescription = runDisclosureLabel }) {
                    LucideIcon(if (runCollapsed) LucideIconKind.ChevronRight else LucideIconKind.ChevronDown, Modifier.size(18.dp))
                }
            }
        }
        if (type == "llm.response") {
            data.str("text").takeIf { it.isNotBlank() }?.let { MarkdownText(it, Modifier.fillMaxWidth().padding(top = 6.dp)) }
            if (showThinking) data.str("thinking").takeIf { it.isNotBlank() }?.let {
                Column(Modifier.fillMaxWidth().padding(top = 6.dp)) {
                    Text(stringResource(Res.string.feature_thinking, ""), style = MaterialTheme.typography.labelSmall)
                    MarkdownText(it, Modifier.fillMaxWidth())
                }
            }
        }
        if (type == "tool.end" && showOutput) (data["details"] as? JsonObject)?.takeIf { it.isNotEmpty() }?.let { details ->
            Text(details.toString(), style = MaterialTheme.typography.bodySmall)
        }
        if (type == "tool.end") {
            val output = data["full_output"] as? JsonObject
            val runId = item.str("run_id")
            val callId = data.str("call_id")
            if (output != null && runId.isNotBlank() && callId.isNotBlank()) {
                val outputKey = "$runId:$callId"
                Button(onClick = { onFetchFull(runId, callId) }) { Text(stringResource(Res.string.feature_trace_full_output)) }
                fullOutputs[outputKey]?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
            }
        }
        val requestId = data.str("request_id")
        val callId = data.str("call_id")
        visibleTraceFragments(fragments, requestId, callId, showThinking, showOutput)
            .forEach { Text(it, style = MaterialTheme.typography.bodySmall) }
        if (type == "takeover") {
            val botId = data.str("bot_id")
            Button(onClick = { onOpenScreen(botId, data.str("tab_id").takeIf { it.isNotBlank() }) }) { Text(stringResource(Res.string.feature_open_screen)) }
        }
    }
}

private fun JsonObject.longValue(key: String): Long? = this[key]?.toString()?.trim('"')?.toLongOrNull()

