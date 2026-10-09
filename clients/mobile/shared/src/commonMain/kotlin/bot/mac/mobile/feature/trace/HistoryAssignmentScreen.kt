package bot.mac.mobile.feature.trace

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
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
import bot.mac.mobile.core.protocol.objects
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.resources.*
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.jetbrains.compose.resources.stringResource

/** Assignment history for a chat or Bot, with server-side cursor pagination. */
@Composable
fun HistoryAssignmentScreen(
    repository: MobileRepository,
    chatId: String? = null,
    botId: String? = null,
    onOpenTrace: (String) -> Unit = {},
    onBack: () -> Unit = {},
) {
    val state by repository.state.collectAsState()
    var status by remember { mutableStateOf<String?>(null) }
    var assignments by remember { mutableStateOf(emptyList<kotlinx.serialization.json.JsonObject>()) }
    var nextCursor by remember { mutableStateOf<String?>(null) }
    var loading by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    val statusOptions = listOf(
        null to stringResource(Res.string.feature_history_all),
        "queued" to stringResource(Res.string.feature_history_queued),
        "working" to stringResource(Res.string.feature_history_working),
        "waiting_user" to stringResource(Res.string.feature_history_waiting_user),
        "waiting_bot" to stringResource(Res.string.feature_history_waiting_bot),
        "blocked" to stringResource(Res.string.feature_history_blocked),
    )

    suspend fun load(reset: Boolean) {
        if (loading) return
        loading = true
        error = null
        runCatching {
            repository.call("assignment.list", buildJsonObject {
                chatId?.let { put("chat_id", it) }
                botId?.let { put("bot_id", it) }
                status?.let { selected ->
                    put("status", buildJsonArray { add(JsonPrimitive(selected)) })
                }
                if (!reset) nextCursor?.let { put("cursor", it) }
                put("limit", 50)
            })
        }.onSuccess { result ->
            val page = result.objects("items")
            assignments = if (reset) page else (assignments + page).distinctBy { it.str("id") }
            nextCursor = result.str("next_cursor").takeIf { it.isNotBlank() }
        }.onFailure { failure -> error = failure.message }
        loading = false
    }

    LaunchedEffect(chatId, botId, status, state.connected) {
        if (state.connected) {
            nextCursor = null
            assignments = emptyList()
            load(reset = true)
        }
    }

    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp)) {
            Button(onClick = onBack) { Text(stringResource(Res.string.feature_back)) }
            Text(stringResource(Res.string.feature_history_title), Modifier.weight(1f).padding(start = 12.dp), style = MaterialTheme.typography.titleLarge)
        }
        Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            statusOptions.forEach { (value, label) ->
                FilterChip(selected = status == value, onClick = { status = value }, label = { Text(label) })
            }
        }
        error?.let { Text(it, Modifier.padding(12.dp), color = MaterialTheme.colorScheme.error) }
        LazyColumn(Modifier.weight(1f).fillMaxWidth().padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            items(assignments, key = { it.str("id").ifBlank { it.hashCode().toString() } }) { assignment ->
                val assignmentId = assignment.str("id")
                val title = assignment.str("title").ifBlank { assignment.str("instruction") }
                    .ifBlank { assignment.str("task") }
                val assignmentStatus = assignment.str("status")
                Column(Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surfaceVariant).padding(12.dp)) {
                    Text(title, style = MaterialTheme.typography.titleMedium)
                    Text(assignmentStatus, style = MaterialTheme.typography.labelMedium)
                    assignment.str("instruction").takeIf { it.isNotBlank() && it != title }?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
                    if (assignmentId.isNotBlank()) {
                        OutlinedButton(onClick = { onOpenTrace(assignmentId) }) {
                            Text(stringResource(Res.string.feature_trace_running))
                        }
                    }
                }
            }
            if (!loading && assignments.isEmpty()) item { Text(stringResource(Res.string.feature_history_empty), Modifier.padding(24.dp)) }
            if (loading) item { Text(stringResource(Res.string.feature_history_loading), Modifier.padding(24.dp)) }
            if (!loading && nextCursor != null) item {
                Button(onClick = { scope.launch { load(reset = false) } }, Modifier.fillMaxWidth()) {
                    Text(stringResource(Res.string.feature_history_load_more))
                }
            }
            if (!loading && assignments.isNotEmpty() && nextCursor == null) item {
                Spacer(Modifier.padding(4.dp))
                Text(stringResource(Res.string.feature_history_no_more), Modifier.fillMaxWidth().padding(8.dp), style = MaterialTheme.typography.labelSmall)
            }
        }
    }
}
