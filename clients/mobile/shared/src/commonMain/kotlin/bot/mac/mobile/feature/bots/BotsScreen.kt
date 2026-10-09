package bot.mac.mobile.feature.bots

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.TextButton
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
import bot.mac.mobile.feature.chat.StatusLabel
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
    var selectedTemplate by remember { mutableStateOf<JsonObject?>(null) }
    var showHidden by remember { mutableStateOf(false) }
    var listError by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    val templateCreateFailed = stringResource(Res.string.feature_template_create_failed)
    LaunchedEffect(showHidden) {
        runCatching { repository.call("bot.list", buildJsonObject { put("include_hidden", showHidden) }) }.onFailure { listError = it.message }
    }
    LaunchedEffect(Unit) {
        runCatching { templates = repository.call("bot.templates", buildJsonObject {}).objects("templates") }.onFailure { listError = it.message }
    }
    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background).padding(14.dp)) {
        Row(Modifier.fillMaxWidth()) {
            Button(onClick = onBack) { Text(stringResource(Res.string.feature_back)) }
            Text(stringResource(Res.string.feature_bot_management), Modifier.weight(1f).padding(start = 12.dp), style = MaterialTheme.typography.titleLarge)
            Button(onClick = onCreate) { Text("＋ ${stringResource(Res.string.feature_new_bot)}") }
        }
        listError?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        Row(Modifier.fillMaxWidth().padding(vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            FilterChip(selected = showHidden, onClick = { showHidden = !showHidden }, label = {
                Text(stringResource(if (showHidden) Res.string.feature_hide_hidden else Res.string.feature_show_hidden))
            })
        }
        if (templates.isNotEmpty()) {
            Text(stringResource(Res.string.feature_templates), Modifier.padding(top = 10.dp), style = MaterialTheme.typography.titleSmall)
            Row(Modifier.fillMaxWidth().padding(vertical = 6.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                templates.forEach { template ->
                    Button(onClick = { selectedTemplate = template }) {
                        Text(template.str("name").ifBlank { template.str("id") })
                    }
                }
            }
        }
        selectedTemplate?.let { template ->
            AlertDialog(
                onDismissRequest = { selectedTemplate = null },
                title = { Text(template.str("name").ifBlank { template.str("id") }) },
                text = {
                    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Text(template.str("description"))
                        Text(stringResource(Res.string.feature_template_confirm))
                    }
                },
                confirmButton = {
                    TextButton(onClick = {
                        scope.launch {
                            runCatching {
                                repository.call("bot.create_from_template", buildJsonObject { put("template_id", template.str("id")) })
                            }.onSuccess { result ->
                                selectedTemplate = null
                                val id = result.objects("bots").firstOrNull()?.str("id")?.takeIf { it.isNotBlank() }
                                    ?: result.str("id").takeIf { it.isNotBlank() }
                                    ?: result.obj("bot").str("id").takeIf { it.isNotBlank() }
                                if (id.isNullOrBlank()) listError = templateCreateFailed else onOpenBot(id)
                            }.onFailure {
                                listError = it.message ?: templateCreateFailed
                            }
                        }
                    }) { Text(stringResource(Res.string.feature_use_template)) }
                },
                dismissButton = { TextButton(onClick = { selectedTemplate = null }) { Text(stringResource(Res.string.common_cancel)) } },
            )
        }
        LazyColumn(Modifier.fillMaxSize().padding(top = 12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            items(state.bots.filter { showHidden || !it.boolean("hidden") }, key = { it.str("id").ifBlank { "bot:${it.hashCode()}" } }) { bot ->
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
            val active = status?.str("active").orEmpty().ifBlank { "0" }
            Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                if (summary.isNotBlank()) StatusLabel(summary) else Text(stringResource(Res.string.feature_idle), style = MaterialTheme.typography.labelSmall)
                Text(stringResource(Res.string.feature_active_count, active), style = MaterialTheme.typography.labelSmall)
            }
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
    var confirmDelete by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    val saveFailed = stringResource(Res.string.feature_save_failed)
    val duplicateName = stringResource(Res.string.feature_copy_suffix, name.trim())
    if (botId != null && existing == null) {
        Column(Modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Button(onClick = onBack) { Text(stringResource(Res.string.feature_back)) }
            Text(stringResource(Res.string.feature_loading), style = MaterialTheme.typography.titleMedium)
        }
        return
    }
    Column(Modifier.fillMaxSize().imePadding().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Row(Modifier.fillMaxWidth()) { Button(onClick = onBack) { Text(stringResource(Res.string.feature_back)) }; Text(if (botId == null) stringResource(Res.string.feature_new_bot) else stringResource(Res.string.feature_edit), Modifier.padding(start = 12.dp), style = MaterialTheme.typography.titleLarge) }
        OutlinedTextField(name, { name = it }, Modifier.fillMaxWidth(), label = { Text(stringResource(Res.string.feature_name)) })
        OutlinedTextField(label, { label = it }, Modifier.fillMaxWidth(), label = { Text(stringResource(Res.string.feature_label)) })
        OutlinedTextField(description, { description = it }, Modifier.fillMaxWidth(), minLines = 3, label = { Text(stringResource(Res.string.feature_description)) })
        OutlinedTextField(model, { model = it }, Modifier.fillMaxWidth(), label = { Text(stringResource(Res.string.feature_model_optional)) })
        if (state.models.isNotEmpty()) {
            Text(stringResource(Res.string.feature_model_choices), style = MaterialTheme.typography.labelSmall)
            Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                state.models.forEach { candidate ->
                    val ref = candidate.str("ref")
                    if (ref.isNotBlank()) {
                        FilterChip(
                            selected = model == ref,
                            onClick = { model = if (model == ref) "" else ref },
                            label = { Text(candidate.str("display_name").ifBlank { ref }) },
                        )
                    }
                }
            }
        }
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
        if (botId != null && !isMainBot) {
            Row(verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) { Text(stringResource(Res.string.feature_hidden), Modifier.weight(1f)); Switch(checked = hidden, onCheckedChange = { hidden = it }) }
        }
        error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
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
                        if (!isMainBot) put("hidden", hidden)
                    }
                    if (botId == null) repository.call("bot.create", payload)
                    else repository.call("bot.update", buildJsonObject { put("bot_id", botId); put("patch", payload) })
                }.onSuccess { onSaved() }.onFailure { error = it.message ?: saveFailed }
            }
        }, modifier = Modifier.fillMaxWidth()) { Text(stringResource(Res.string.feature_save)) }
        if (botId != null && !isMainBot) {
            Button(onClick = { scope.launch { runCatching { repository.call("bot.duplicate", buildJsonObject { put("bot_id", botId); put("name", duplicateName) }) }.onSuccess { onSaved() }.onFailure { error = it.message ?: saveFailed } } }, modifier = Modifier.fillMaxWidth()) { Text(stringResource(Res.string.feature_duplicate)) }
            Button(onClick = { confirmDelete = true }, modifier = Modifier.fillMaxWidth()) { Text(stringResource(Res.string.feature_delete_bot)) }
        }
    }
    if (confirmDelete) {
        AlertDialog(
            onDismissRequest = { confirmDelete = false },
            title = { Text(stringResource(Res.string.feature_delete_bot)) },
            text = { Text(stringResource(Res.string.feature_delete_confirm, name.trim())) },
            confirmButton = {
                TextButton(onClick = {
                    confirmDelete = false
                    scope.launch { runCatching { repository.call("bot.delete", buildJsonObject { put("bot_id", botId) }) }.onSuccess { onBack() }.onFailure { error = it.message ?: saveFailed } }
                }) { Text(stringResource(Res.string.feature_delete_confirm_action)) }
            },
            dismissButton = { TextButton(onClick = { confirmDelete = false }) { Text(stringResource(Res.string.common_cancel)) } },
        )
    }
}

private fun avatarJson(emoji: String) = buildJsonObject {
    if (emoji.isBlank()) { put("kind", "bean"); put("color", 0) } else { put("kind", "emoji"); put("emoji", emoji.trim()) }
}

private fun toolsJson(tools: Map<String, Boolean>) = buildJsonObject { tools.forEach { (key, value) -> put(key, value) } }

