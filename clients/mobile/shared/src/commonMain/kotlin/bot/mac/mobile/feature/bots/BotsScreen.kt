package bot.mac.mobile.feature.bots

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.FilterChip
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
import bot.mac.mobile.core.protocol.obj
import bot.mac.mobile.core.protocol.objects
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.resources.*
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.jetbrains.compose.resources.stringResource

@Composable
fun BotsScreen(
    repository: MobileRepository,
    onOpenBot: (String) -> Unit = {},
    onCreate: () -> Unit = {},
    onBack: () -> Unit = {},
) {
    val state by repository.state.collectAsState()
    var templates by remember { mutableStateOf(emptyList<JsonObject>()) }
    var listError by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    LaunchedEffect(Unit) {
        runCatching { repository.call("bot.list", buildJsonObject { put("include_hidden", false) }) }.onFailure { listError = it.message }
        runCatching { templates = repository.call("bot.templates", buildJsonObject {}).objects("templates") }.onFailure { listError = it.message }
    }
    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background).padding(14.dp)) {
        Row(Modifier.fillMaxWidth()) {
            Button(onClick = onBack) { Text(stringResource(Res.string.feature_back)) }
            Text(stringResource(Res.string.feature_bot_management), Modifier.weight(1f).padding(start = 12.dp), style = MaterialTheme.typography.titleLarge)
            Button(onClick = onCreate) { Text("＋ ${stringResource(Res.string.feature_create)}") }
        }
        listError?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        if (templates.isNotEmpty()) {
            Text(stringResource(Res.string.feature_templates), Modifier.padding(top = 10.dp), style = MaterialTheme.typography.titleSmall)
            Row(Modifier.fillMaxWidth().padding(vertical = 6.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                templates.forEach { template ->
                    Button(onClick = { scope.launch { runCatching { repository.call("bot.create_from_template", buildJsonObject { put("template_id", template.str("id")) }) }.onFailure { listError = it.message } } }) {
                        Text(template.str("name").ifBlank { template.str("id") })
                    }
                }
            }
        }
        LazyColumn(Modifier.fillMaxSize().padding(top = 12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            items(state.bots.filter { !it.boolean("hidden") }, key = { it.str("id").ifBlank { "bot:${it.hashCode()}" } }) { bot ->
                BotRow(bot, onOpenBot)
            }
        }
    }
}

@Composable
private fun BotRow(bot: JsonObject, onOpen: (String) -> Unit) {
    val id = bot.str("id")
    val status = bot.obj("status")
    Row(Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surfaceVariant).padding(12.dp), verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
        Text(bot.str("name").ifBlank { id }, Modifier.weight(1f), style = MaterialTheme.typography.titleMedium)
        Column(Modifier.weight(1f)) {
            Text(bot.str("label"))
            val summary = status?.str("summary").orEmpty()
            val summaryLabel = if (summary.isBlank()) stringResource(Res.string.feature_idle) else summary
            val active = status?.str("active").orEmpty().ifBlank { "0" }
            Text(stringResource(Res.string.feature_assignment_count, summaryLabel, active), style = MaterialTheme.typography.labelSmall)
        }
        if (bot.boolean("is_main")) Text(stringResource(Res.string.feature_bot_main), style = MaterialTheme.typography.labelSmall)
        Button(onClick = { onOpen(id) }) { Text(stringResource(Res.string.feature_edit)) }
    }
}

@Composable
fun BotEditorScreen(
    repository: MobileRepository,
    botId: String? = null,
    onSaved: () -> Unit = {},
    onBack: () -> Unit = {},
) {
    val state by repository.state.collectAsState()
    val existing = state.bots.firstOrNull { it.str("id") == botId }
    val isMainBot = existing?.boolean("is_main") == true
    var name by remember(botId) { mutableStateOf(existing?.str("name") ?: "") }
    var label by remember(botId) { mutableStateOf(existing?.str("label") ?: "") }
    var description by remember(botId) { mutableStateOf(existing?.str("description") ?: "") }
    var model by remember(botId) { mutableStateOf(existing?.str("model") ?: "") }
    var maxParallel by remember(botId) { mutableStateOf(existing?.str("max_parallel") ?: "1") }
    var browser by remember(botId) { mutableStateOf(existing?.str("browser_mode") ?: "headless") }
    var pinned by remember(botId) { mutableStateOf(existing?.boolean("pinned") == true) }
    var hidden by remember(botId) { mutableStateOf(existing?.boolean("hidden") == true) }
    var notifications by remember(botId) { mutableStateOf(existing?.boolean("notifications") != false) }
    var emoji by remember(botId) { mutableStateOf(existing?.obj("avatar")?.str("emoji") ?: "") }
    var tools by remember(botId) { mutableStateOf(mapOf("files" to true, "bash" to true, "browser" to true, "subagent" to false, "web" to true, "mcp" to false).mapValues { (key, fallback) -> existing?.obj("tools")?.boolean(key) ?: fallback }) }
    var error by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    val saveFailed = stringResource(Res.string.feature_save_failed)
    val duplicateName = stringResource(Res.string.feature_copy_suffix, name.trim())
    Column(Modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Row(Modifier.fillMaxWidth()) { Button(onClick = onBack) { Text(stringResource(Res.string.feature_back)) }; Text(if (botId == null) stringResource(Res.string.feature_new_bot) else stringResource(Res.string.feature_edit), Modifier.padding(start = 12.dp), style = MaterialTheme.typography.titleLarge) }
        OutlinedTextField(name, { name = it }, Modifier.fillMaxWidth(), label = { Text(stringResource(Res.string.feature_name)) })
        OutlinedTextField(label, { label = it }, Modifier.fillMaxWidth(), label = { Text(stringResource(Res.string.feature_label)) })
        OutlinedTextField(description, { description = it }, Modifier.fillMaxWidth(), minLines = 3, label = { Text(stringResource(Res.string.feature_description)) })
        OutlinedTextField(model, { model = it }, Modifier.fillMaxWidth(), label = { Text(stringResource(Res.string.feature_model_optional)) })
        if (!isMainBot) {
            OutlinedTextField(maxParallel, { maxParallel = it.filter(Char::isDigit) }, Modifier.fillMaxWidth(), label = { Text(stringResource(Res.string.feature_parallel_limit)) })
            OutlinedTextField(browser, { browser = it }, Modifier.fillMaxWidth(), label = { Text(stringResource(Res.string.feature_browser_mode)) })
            Text(stringResource(Res.string.feature_tools), style = MaterialTheme.typography.titleSmall)
            listOf("files" to Res.string.feature_files, "bash" to Res.string.feature_bash, "browser" to Res.string.feature_browser, "subagent" to Res.string.feature_subagent, "web" to Res.string.feature_web, "mcp" to Res.string.feature_mcp).forEach { (key, title) ->
                Row(verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
                    Text(stringResource(title), Modifier.weight(1f))
                    Switch(checked = tools[key] == true, onCheckedChange = { value -> tools = tools + (key to value) })
                }
            }
        }
        Text(stringResource(Res.string.feature_avatar), style = MaterialTheme.typography.titleSmall)
        OutlinedTextField(emoji, { emoji = it }, Modifier.fillMaxWidth(), label = { Text(stringResource(Res.string.feature_emoji)) })
        Row(verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) { Text(stringResource(Res.string.feature_pin), Modifier.weight(1f)); Switch(checked = pinned, onCheckedChange = { pinned = it }) }
        Row(verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) { Text(stringResource(Res.string.feature_bot_notifications), Modifier.weight(1f)); Switch(checked = notifications, onCheckedChange = { notifications = it }) }
        if (botId != null && existing?.boolean("is_main") != true) {
            Row(verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) { Text(stringResource(Res.string.feature_hidden), Modifier.weight(1f)); Switch(checked = hidden, onCheckedChange = { hidden = it }) }
        }
        error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        Spacer(Modifier.weight(1f))
        Button(enabled = name.isNotBlank(), onClick = {
            scope.launch {
                runCatching {
                    val payload = buildJsonObject {
                        put("name", name.trim())
                        put("label", label.trim())
                        put("description", description.trim())
                        put("avatar", avatarJson(emoji))
                        put("model", model.ifBlank { null })
                        if (!isMainBot) {
                            put("max_parallel", maxParallel.toIntOrNull() ?: 1)
                            put("tools", toolsJson(tools))
                            put("browser_mode", browser)
                        }
                        put("notifications", notifications)
                        put("pinned", pinned)
                        put("hidden", hidden)
                    }
                    if (botId == null) repository.call("bot.create", payload)
                    else repository.call("bot.update", buildJsonObject { put("bot_id", botId); put("patch", payload) })
                }.onSuccess { onSaved() }.onFailure { error = it.message ?: saveFailed }
            }
        }, modifier = Modifier.fillMaxWidth()) { Text(stringResource(Res.string.feature_save)) }
        if (botId != null && existing?.boolean("is_main") != true) {
            Button(onClick = { scope.launch { runCatching { repository.call("bot.duplicate", buildJsonObject { put("bot_id", botId); put("name", duplicateName) }) }.onSuccess { onSaved() }.onFailure { error = it.message ?: saveFailed } } }, modifier = Modifier.fillMaxWidth()) { Text(stringResource(Res.string.feature_duplicate)) }
            Button(onClick = { scope.launch { runCatching { repository.call("bot.delete", buildJsonObject { put("bot_id", botId) }) }.onSuccess { onBack() }.onFailure { error = it.message ?: saveFailed } } }, modifier = Modifier.fillMaxWidth()) { Text(stringResource(Res.string.feature_delete_bot)) }
        }
    }
}

private fun avatarJson(emoji: String) = buildJsonObject {
    if (emoji.isBlank()) { put("kind", "bean"); put("color", 0) } else { put("kind", "emoji"); put("emoji", emoji.trim()) }
}

private fun toolsJson(tools: Map<String, Boolean>) = buildJsonObject { tools.forEach { (key, value) -> put(key, value) } }

