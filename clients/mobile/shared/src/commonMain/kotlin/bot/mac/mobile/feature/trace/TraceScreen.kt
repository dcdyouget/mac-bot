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
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.protocol.boolean
import bot.mac.mobile.core.protocol.objects
import bot.mac.mobile.core.protocol.obj
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.core.ui.MarkdownText
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
    val fragments = state.traceFragments.filterKeys { it.startsWith("${stream ?: streamKey}:") }
    val scope = rememberCoroutineScope()
    val activeFilter = filter
    val traceError = stringResource(Res.string.feature_trace_error)

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
        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp)) {
            Button(onClick = onBack) { Text(stringResource(Res.string.feature_back)) }
            Text(stringResource(Res.string.feature_trace), Modifier.weight(1f).padding(start = 12.dp), style = MaterialTheme.typography.titleLarge)
            if (live) Text(stringResource(Res.string.feature_realtime), color = MaterialTheme.colorScheme.primary, modifier = Modifier.padding(8.dp))
        }
        Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            TraceFilter.entries.forEach { candidate ->
                FilterChip(selected = filter == candidate, onClick = { filter = if (filter == candidate) null else candidate }, label = { Text(candidate.label()) })
            }
        }
        Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlinedTextField(search, { search = it }, Modifier.weight(1f), singleLine = true, placeholder = { Text(stringResource(Res.string.feature_trace_search)) })
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
                    onToggleRun = { runId -> collapsedRuns = if (runId in collapsedRuns) collapsedRuns - runId else collapsedRuns + runId },
                    onFetchFull = { path -> scope.launch { runCatching { repository.fetchText(path, emptyMap()) }.onSuccess { fullOutputs = fullOutputs + (path to it) }.onFailure { error = it.message } } })
            }
            if (items.isEmpty() && fragments.isEmpty()) item { Text(stringResource(Res.string.feature_no_trace), Modifier.padding(24.dp)) }
        }
        if (assignmentId != null) {
            Row(Modifier.fillMaxWidth().padding(12.dp), horizontalArrangement = Arrangement.End) {
                Button(onClick = { scope.launch { runCatching { repository.call("assignment.stop", buildJsonObject { put("assignment_id", assignmentId) }) }.onFailure { error = it.message } } }) { Text(stringResource(Res.string.feature_stop)) }
            }
        }
    }
}

private enum class TraceFilter {
    ALL, MODEL, TOOL, MESSAGE, WAIT;
    @Composable fun label(): String = when (this) {
        ALL -> stringResource(Res.string.feature_trace_all)
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
    onToggleRun: (String) -> Unit,
    onFetchFull: (String) -> Unit,
) {
    val type = item.str("type").ifBlank { "event" }
    val data = item.obj("data")
    val summary = when (type) {
        "run.start" -> stringResource(Res.string.feature_trace_start, data?.str("phase") ?: "")
        "run.end" -> stringResource(Res.string.feature_trace_end, data?.str("status") ?: "")
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
            Text(summary, Modifier.weight(1f), style = MaterialTheme.typography.titleSmall)
            Text(item.str("at"), style = MaterialTheme.typography.labelSmall)
            if (type == "run.start" && item.str("run_id").isNotBlank()) Button(onClick = { onToggleRun(item.str("run_id")) }) { Text("▾") }
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
        if (type == "tool.end" && showOutput) data.str("details").takeIf { it.isNotBlank() }?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
        if (type == "tool.end") {
            val outputPath = data.obj("full_output").str("path").ifBlank { data.obj("full_output").str("url") }
            if (outputPath.isNotBlank()) {
                Button(onClick = { onFetchFull(outputPath) }) { Text(stringResource(Res.string.feature_trace_full_output)) }
                fullOutputs[outputPath]?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
            }
        }
        val runId = item.str("run_id")
        if (runId.isNotBlank()) fragments.filterKeys { it.contains(runId) }.values.forEach { Text(it, style = MaterialTheme.typography.bodySmall) }
        if (type == "takeover") {
            val botId = data.str("bot_id")
            Button(onClick = { onOpenScreen(botId, data.str("tab_id").takeIf { it.isNotBlank() }) }) { Text(stringResource(Res.string.feature_open_screen)) }
        }
    }
}

private fun JsonObject.longValue(key: String): Long? = this[key]?.toString()?.trim('"')?.toLongOrNull()

