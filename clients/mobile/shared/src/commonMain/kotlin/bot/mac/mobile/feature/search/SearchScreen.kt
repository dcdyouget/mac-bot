package bot.mac.mobile.feature.search

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Card
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.protocol.arr
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.resources.Res
import bot.mac.mobile.resources.search_error
import bot.mac.mobile.resources.search_all
import bot.mac.mobile.resources.search_artifact
import bot.mac.mobile.resources.search_bot
import bot.mac.mobile.resources.search_chat
import bot.mac.mobile.resources.search_empty
import bot.mac.mobile.resources.search_hint
import bot.mac.mobile.resources.search_message
import bot.mac.mobile.resources.search_no_results
import bot.mac.mobile.resources.search_routine
import bot.mac.mobile.resources.search_title
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.coroutines.CancellationException
import org.jetbrains.compose.resources.stringResource

private enum class SearchKind { ALL, MESSAGE, CHAT, BOT, ARTIFACT, ROUTINE }

@Composable
fun SearchScreen(repository: MobileRepository, onOpenResult: (JsonObject) -> Unit, onBack: () -> Unit) {
    var query by remember { mutableStateOf("") }
    var kind by remember { mutableStateOf(SearchKind.ALL) }
    var results by remember { mutableStateOf<List<JsonObject>>(emptyList()) }
    var searching by remember { mutableStateOf(false) }
    var searchError by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    var pending by remember { mutableStateOf<Job?>(null) }

    fun search() {
        pending?.cancel()
        pending = scope.launch {
            delay(220)
            if (query.isBlank()) { results = emptyList(); searching = false; return@launch }
            searching = true
            searchError = null
            try {
                val response = repository.call("search", buildJsonObject {
                    put("query", query.trim())
                    if (kind != SearchKind.ALL) put("kinds", JsonArray(listOf(kindsJson(kind))))
                    put("limit", 40)
                })
                results = response.arr("results").orEmpty().mapNotNull { it as? JsonObject }
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (failure: Throwable) {
                results = emptyList()
                searchError = failure.message ?: ""
            } finally {
                searching = false
            }
        }
    }
    LaunchedEffect(query, kind) { search() }

    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = onBack) { Text("‹", style = MaterialTheme.typography.headlineSmall) }
            Text(stringResource(Res.string.search_title), Modifier.weight(1f), style = MaterialTheme.typography.titleLarge)
        }
        OutlinedTextField(query, { query = it }, Modifier.fillMaxWidth().padding(horizontal = 12.dp), singleLine = true, placeholder = { Text(stringResource(Res.string.search_hint)) })
        Row(Modifier.fillMaxWidth().padding(12.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            SearchKind.entries.forEach { candidate -> FilterChip(kind == candidate, { kind = candidate }, label = { Text(candidate.label()) }) }
        }
        HorizontalDivider()
        when {
            query.isBlank() -> Text(stringResource(Res.string.search_empty), Modifier.padding(24.dp))
            searching -> Text("…", Modifier.padding(24.dp))
            searchError != null -> Text(stringResource(Res.string.search_error, searchError!!), Modifier.padding(24.dp), color = MaterialTheme.colorScheme.error)
            results.isEmpty() -> Text(stringResource(Res.string.search_no_results), Modifier.padding(24.dp))
            else -> LazyColumn(Modifier.fillMaxSize().padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                items(results, key = { "${it.str("kind")}:${it.str("id")}" }) { result -> SearchResult(result, onClick = { onOpenResult(result) }) }
            }
        }
    }
}

@Composable
private fun SearchResult(result: JsonObject, onClick: () -> Unit) {
    Card(onClick = onClick, modifier = Modifier.fillMaxWidth()) {
        Column(Modifier.padding(12.dp)) {
            Text(kindLabel(result.str("kind") ?: ""), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.primary)
            Text(result.str("title") ?: result.str("id") ?: "", style = MaterialTheme.typography.titleSmall)
            result.str("snippet")?.takeIf { it.isNotBlank() }?.let { Text(it, style = MaterialTheme.typography.bodyMedium) }
            result.str("at")?.let { Text(it, style = MaterialTheme.typography.labelSmall) }
        }
    }
}

internal fun searchKindParameter(kind: String): String? = when (kind) {
    "message", "chat", "bot", "artifact", "routine" -> kind
    else -> null
}
private fun kindsJson(kind: SearchKind): kotlinx.serialization.json.JsonPrimitive = kotlinx.serialization.json.JsonPrimitive(kind.name.lowercase())
@Composable private fun SearchKind.label(): String = when (this) {
    SearchKind.ALL -> stringResource(Res.string.search_all)
    SearchKind.MESSAGE -> stringResource(Res.string.search_message)
    SearchKind.CHAT -> stringResource(Res.string.search_chat)
    SearchKind.BOT -> stringResource(Res.string.search_bot)
    SearchKind.ARTIFACT -> stringResource(Res.string.search_artifact)
    SearchKind.ROUTINE -> stringResource(Res.string.search_routine)
}
@Composable private fun kindLabel(kind: String): String = when (kind) {
    "message" -> stringResource(Res.string.search_message)
    "chat" -> stringResource(Res.string.search_chat)
    "bot" -> stringResource(Res.string.search_bot)
    "artifact" -> stringResource(Res.string.search_artifact)
    "routine" -> stringResource(Res.string.search_routine)
    else -> stringResource(Res.string.search_all)
}
