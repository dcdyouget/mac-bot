package bot.mac.mobile.feature.bots

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.clickable
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.ui.draw.clip
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
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.design.BotAvatar
import bot.mac.mobile.core.protocol.arr
import bot.mac.mobile.core.protocol.long
import bot.mac.mobile.core.protocol.boolean
import bot.mac.mobile.core.protocol.obj
import bot.mac.mobile.core.protocol.objects
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.design.DesignTokens
import bot.mac.mobile.core.platform.platformFilePicker
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
import kotlin.time.Clock
import kotlin.time.Instant

@Composable
fun BotsScreen(
    repository: MobileRepository,
    onOpenBot: (String) -> Unit = {},
    onCreate: () -> Unit = {},
    onBack: () -> Unit = {},
    onOpenDetail: (String) -> Unit = onOpenBot,
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
    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            Button(onClick = onBack) { Text(stringResource(Res.string.feature_back)) }
            Text(stringResource(Res.string.feature_bot_management), Modifier.weight(1f).padding(start = 12.dp), style = MaterialTheme.typography.titleLarge)
            Button(onClick = onCreate) { Text("＋ ${stringResource(Res.string.feature_new_bot)}") }
        }
        listError?.let { Text(it, color = MaterialTheme.colorScheme.error, modifier = Modifier.padding(horizontal = 16.dp)) }
        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
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
        val visibleBots = state.bots.filter { showHidden || !it.boolean("hidden") }
        val mainBot = visibleBots.firstOrNull { it.boolean("is_main") }
        val regularBots = visibleBots.filterNot { it.boolean("is_main") }
        LazyColumn(Modifier.fillMaxSize().padding(horizontal = 16.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            if (mainBot != null) {
                item { Text(stringResource(Res.string.feature_bot_main), style = MaterialTheme.typography.titleSmall, modifier = Modifier.padding(start = 4.dp, top = 8.dp, bottom = 2.dp)) }
                item { BotRow(repository, mainBot, onOpenDetail, onOpenBot) }
            }
            if (regularBots.isNotEmpty()) {
                item { Text(stringResource(Res.string.feature_bot), style = MaterialTheme.typography.titleSmall, modifier = Modifier.padding(start = 4.dp, top = 12.dp, bottom = 2.dp)) }
                items(regularBots, key = { it.str("id").ifBlank { "bot:${it.hashCode()}" } }) { bot ->
                    BotRow(repository, bot, onOpenDetail, onOpenBot)
                }
            }
        }
    }
}

@Composable
private fun BotRow(repository: MobileRepository, bot: JsonObject, onOpen: (String) -> Unit, onEdit: (String) -> Unit) {
    val id = bot.str("id")
    val status = bot.obj("status")
    Row(
        Modifier.fillMaxWidth().height(68.dp).clip(MaterialTheme.shapes.medium)
            .background(MaterialTheme.colorScheme.surfaceVariant).clickable { onOpen(id) }.padding(horizontal = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Box(Modifier.size(44.dp), contentAlignment = Alignment.Center) {
            BotAvatar(repository, bot, main = bot.boolean("is_main"), status = status?.str("summary").orEmpty())
        }
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(bot.str("name").ifBlank { id }, style = MaterialTheme.typography.titleMedium, maxLines = 1)
            Text(bot.str("label").ifBlank { bot.str("description") }, style = MaterialTheme.typography.bodySmall, maxLines = 1)
        }
        Column(horizontalAlignment = Alignment.End) {
            val summary = status?.str("summary").orEmpty()
            val active = status?.long("active") ?: 0L
            if (summary.isNotBlank()) StatusLabel(summary) else Text(stringResource(Res.string.feature_idle), style = MaterialTheme.typography.labelSmall)
            if (active > 0) Text(stringResource(Res.string.feature_active_count, active.toString()), style = MaterialTheme.typography.labelSmall)
        }
        TextButton(onClick = { onEdit(id) }) { Text(stringResource(Res.string.feature_edit)) }
    }
}

/** Bot profile with live group, assignment history, and usage data from protocol RPCs. */
@Composable
fun BotDetailScreen(
    repository: MobileRepository,
    botId: String,
    onOpenChat: (String) -> Unit = {},
    onOpenScreen: (String) -> Unit = {},
    onOpenRoutines: (String) -> Unit = {},
    onOpenHistory: (String) -> Unit = {},
    onEdit: (String) -> Unit = {},
    onBack: () -> Unit = {},
) {
    val state by repository.state.collectAsState()
    val bot = state.bots.firstOrNull { it.str("id") == botId }
    var groups by remember(botId) { mutableStateOf(emptyList<JsonObject>()) }
    var assignments by remember(botId) { mutableStateOf(emptyList<JsonObject>()) }
    var usage by remember(botId) { mutableStateOf<JsonObject?>(null) }
    var error by remember(botId) { mutableStateOf<String?>(null) }
    LaunchedEffect(botId, state.connected) {
        if (!state.connected) return@LaunchedEffect
        runCatching {
            repository.call("bot.get", buildJsonObject { put("bot_id", botId) })
            val projects = repository.call("project.list", buildJsonObject {}).objects("projects")
            val history = repository.call("assignment.list", buildJsonObject { put("bot_id", botId); put("limit", 5) }).objects("items")
            val now = Clock.System.now()
            val from = Instant.fromEpochMilliseconds(now.toEpochMilliseconds() - 7L * 86_400_000L).toString()
            val usageRows = repository.call("usage.breakdown", buildJsonObject {
                put("from", from); put("to", now.toString()); put("dimension", "bot")
            }).objects("rows")
            groups = projects.filter { project -> project.arr("members").any { (it as? JsonObject)?.str("bot_id") == botId } }
            assignments = history
            usage = usageRows.firstOrNull { it.str("key") == botId || it.str("id") == botId }?.obj("usage")
        }.onFailure { error = it.message ?: "" }
    }
    if (bot == null) {
        Column(Modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Button(onClick = onBack) { Text(stringResource(Res.string.feature_back)) }
            Text(stringResource(Res.string.feature_loading), style = MaterialTheme.typography.titleMedium)
        }
        return
    }
    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            Button(onClick = onBack) { Text(stringResource(Res.string.feature_back)) }
            Text(bot.str("name"), Modifier.weight(1f).padding(start = 12.dp), style = MaterialTheme.typography.titleLarge, maxLines = 1)
            TextButton(onClick = { onEdit(botId) }) { Text(stringResource(Res.string.feature_edit)) }
        }
        error?.let { Text(it, color = MaterialTheme.colorScheme.error, modifier = Modifier.padding(horizontal = 16.dp)) }
        LazyColumn(Modifier.fillMaxSize().padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            item {
                Column(Modifier.fillMaxWidth().padding(vertical = 12.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    Box(Modifier.size(72.dp), contentAlignment = Alignment.Center) { BotAvatar(repository, bot) }
                    Text(bot.str("name"), style = MaterialTheme.typography.titleLarge)
                    Text(listOf(bot.str("label"), bot.str("model")).filter(String::isNotBlank).joinToString(" · "), style = MaterialTheme.typography.bodySmall)
                    Row(Modifier.fillMaxWidth().padding(top = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        DetailShortcut(Modifier.weight(1f), stringResource(Res.string.feature_open_chat), onClick = { onOpenChat(bot.str("dm_chat_id")) })
                        DetailShortcut(Modifier.weight(1f), stringResource(Res.string.feature_open_screen), onClick = { onOpenScreen(botId) })
                        DetailShortcut(Modifier.weight(1f), stringResource(Res.string.nav_routines), onClick = { onOpenRoutines(botId) })
                        DetailShortcut(Modifier.weight(1f), stringResource(Res.string.feature_settings), onClick = { onEdit(botId) })
                    }
                }
            }
            item { Text(stringResource(Res.string.feature_group), style = MaterialTheme.typography.titleSmall, modifier = Modifier.padding(start = 4.dp)) }
            if (groups.isEmpty()) item { Text(stringResource(Res.string.feature_history_empty), Modifier.padding(12.dp), style = MaterialTheme.typography.bodySmall) }
            items(groups, key = { it.str("id") }) { project ->
                Row(Modifier.fillMaxWidth().height(56.dp).background(MaterialTheme.colorScheme.surfaceVariant).padding(horizontal = 14.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text(project.str("name"), Modifier.weight(1f), style = MaterialTheme.typography.bodyLarge)
                    Text(project.arr("members").mapNotNull { it as? JsonObject }.firstOrNull { it.str("bot_id") == botId }?.str("state").orEmpty(), style = MaterialTheme.typography.bodySmall)
                }
            }
            item { Text(stringResource(Res.string.feature_history_title), Modifier.padding(start = 4.dp, top = 6.dp).clickable { onOpenHistory(botId) }, style = MaterialTheme.typography.titleSmall) }
            if (assignments.isEmpty()) item { Text(stringResource(Res.string.feature_history_empty), Modifier.padding(12.dp), style = MaterialTheme.typography.bodySmall) }
            items(assignments, key = { it.str("id") }) { assignment ->
                Row(Modifier.fillMaxWidth().height(52.dp).background(MaterialTheme.colorScheme.surfaceVariant).padding(horizontal = 14.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text(assignment.str("title").ifBlank { assignment.str("instruction") }.ifBlank { assignment.str("status") }, Modifier.weight(1f), maxLines = 1)
                    Text(assignment.str("status"), style = MaterialTheme.typography.bodySmall)
                }
            }
            item { Text(stringResource(Res.string.dashboard_title), style = MaterialTheme.typography.titleSmall, modifier = Modifier.padding(start = 4.dp, top = 6.dp)) }
            item {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    val tokens = (usage?.long("input_tokens") ?: 0L) + (usage?.long("output_tokens") ?: 0L)
                    UsageStat(tokens.toString(), Modifier.weight(1f))
                    UsageStat(usage?.get("cost")?.toString()?.trim('"') ?: "—", Modifier.weight(1f))
                }
            }
        }
    }
}

@Composable
private fun DetailShortcut(modifier: Modifier, label: String, onClick: () -> Unit) {
    Column(modifier.height(64.dp).clip(MaterialTheme.shapes.medium).background(MaterialTheme.colorScheme.surfaceVariant).clickable(onClick = onClick), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.Center) {
        Text(label, color = MaterialTheme.colorScheme.primary, style = MaterialTheme.typography.labelMedium, maxLines = 1)
    }
}

@Composable
private fun UsageStat(value: String, modifier: Modifier = Modifier) {
    Column(modifier.height(72.dp).background(MaterialTheme.colorScheme.surfaceVariant).padding(12.dp), verticalArrangement = Arrangement.Center) {
        Text(value, style = MaterialTheme.typography.titleMedium)
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
    val leastUsedAvatarColor = remember(state.bots, botId) {
        (0..9).minWithOrNull(compareBy<Int> { color ->
            state.bots.count { bot -> bot.str("id") != botId && bot.obj("avatar").str("kind") == "bean" && bot.obj("avatar").str("color").toIntOrNull() == color }
        }.thenBy { it }) ?: 0
    }
    var avatarColor by remember(botId, leastUsedAvatarColor) {
        mutableStateOf(if (isMainBot) 0 else existing?.obj("avatar")?.str("color")?.toIntOrNull() ?: leastUsedAvatarColor)
    }
    var avatarFile by remember(botId) {
        mutableStateOf(existing?.obj("avatar")?.takeIf { it.str("kind") == "image" }?.obj("file"))
    }
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
        OutlinedTextField(emoji, { emoji = it; if (it.isNotBlank()) avatarFile = null }, Modifier.fillMaxWidth(), label = { Text(stringResource(Res.string.feature_emoji)) })
        if (!isMainBot) {
            Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                DesignTokens.avatarColors.forEachIndexed { colorIndex, color ->
                    val colorA11y = stringResource(Res.string.feature_avatar_color, colorIndex + 1)
                    androidx.compose.foundation.layout.Box(
                        modifier = Modifier.size(48.dp)
                            .semantics { contentDescription = colorA11y; selected = avatarColor == colorIndex }
                            .clickable { avatarColor = colorIndex; avatarFile = null; emoji = "" },
                        contentAlignment = androidx.compose.ui.Alignment.Center,
                    ) {
                        androidx.compose.foundation.layout.Box(
                            Modifier.size(28.dp).clip(CircleShape).background(color)
                                .then(if (avatarColor == colorIndex) Modifier.border(2.dp, MaterialTheme.colorScheme.primary, CircleShape) else Modifier),
                        )
                    }
                }
            }
            val invalidImage = stringResource(Res.string.feature_avatar_image_invalid)
            Button(onClick = {
                scope.launch {
                    runCatching {
                        val picked = platformFilePicker().pickFile() ?: return@launch
                        require(picked.mimeType.orEmpty().startsWith("image/")) { invalidImage }
                        avatarFile = repository.uploadFile(picked).obj("file")
                        emoji = ""
                    }.onFailure { error = it.message ?: invalidImage }
                }
            }, modifier = Modifier.fillMaxWidth()) { Text(stringResource(Res.string.feature_avatar_image)) }
        }
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
                        put("avatar", avatarJson(emoji, if (isMainBot) 0 else avatarColor, avatarFile))
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

private fun avatarJson(emoji: String, color: Int, file: JsonObject?) = buildJsonObject {
    when {
        file != null -> { put("kind", "image"); put("file", file) }
        emoji.isNotBlank() -> { put("kind", "emoji"); put("emoji", emoji.trim()) }
        else -> { put("kind", "bean"); put("color", color) }
    }
}

private fun toolsJson(tools: Map<String, Boolean>) = buildJsonObject { tools.forEach { (key, value) -> put(key, value) } }

