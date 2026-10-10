package bot.mac.mobile.feature.skills

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
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
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.protocol.arr
import bot.mac.mobile.core.protocol.boolean
import bot.mac.mobile.core.protocol.long
import bot.mac.mobile.core.protocol.obj
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.platform.PickedFile
import bot.mac.mobile.core.platform.PlatformBackHandler
import bot.mac.mobile.core.platform.platformFilePicker
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.resources.*
import kotlinx.coroutines.launch
import kotlinx.coroutines.CancellationException
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.jetbrains.compose.resources.stringResource

private enum class SkillFilter { ALL, BUILTIN, MINE, IMPORTED, DRAFT }

private suspend fun <T> skillRequest(onError: (String) -> Unit, block: suspend () -> T): T? = try {
    block()
} catch (cancelled: CancellationException) {
    throw cancelled
} catch (failure: Throwable) {
    onError(failure.message.orEmpty())
    null
}

/** Applies a skill mutation, refreshes the list cache, then reads the full detail. */
internal suspend fun mutateSkillAndRefresh(
    repository: MobileRepository,
    name: String,
    method: String,
    params: JsonObject,
): JsonObject {
    repository.call(method, params)
    repository.call("skill.list")
    return repository.call("skill.get", buildJsonObject { put("name", name) })
        .obj("skill")
        .takeIf { it.str("name").isNotBlank() }
        ?: error("skill.get returned no detail for $name")
}

internal suspend fun saveSkillAndRefresh(
    repository: MobileRepository,
    existingName: String?,
    name: String,
    description: String,
    content: String,
): JsonObject = mutateSkillAndRefresh(
    repository = repository,
    name = existingName ?: name.trim(),
    method = if (existingName == null) "skill.create" else "skill.update",
    params = buildJsonObject {
        put("name", existingName ?: name.trim())
        put("content", skillContent(existingName ?: name.trim(), description, content))
    },
)

internal suspend fun publishSkillAndRefresh(repository: MobileRepository, name: String): JsonObject =
    mutateSkillAndRefresh(
        repository,
        name,
        "skill.publish",
        buildJsonObject { put("name", name) },
    )

@Composable
fun SkillsScreen(repository: MobileRepository, onBack: () -> Unit) {
    val state by repository.state.collectAsState()
    var selectedName by remember { mutableStateOf<String?>(null) }
    var search by remember { mutableStateOf("") }
    var filter by remember { mutableStateOf(SkillFilter.ALL) }
    var showEditor by remember { mutableStateOf(false) }
    var showImport by remember { mutableStateOf(false) }
    var actionError by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    val skills = state.skills.filter { skill ->
        val source = skill.str("source") ?: "user"
        (search.isBlank() || skill.str("name")?.contains(search, true) == true || skill.str("description")?.contains(search, true) == true) && when (filter) {
            SkillFilter.ALL -> true
            SkillFilter.BUILTIN -> source == "builtin"
            SkillFilter.MINE -> source == "user"
            SkillFilter.IMPORTED -> source == "imported"
            SkillFilter.DRAFT -> source == "draft"
        }
    }
    var detail by remember { mutableStateOf<JsonObject?>(null) }

    LaunchedEffect(Unit) {
        skillRequest({ actionError = it }) { repository.call("skill.list") }
    }
    LaunchedEffect(selectedName) {
        selectedName?.let { name ->
            skillRequest({ actionError = it }) { repository.call("skill.get", buildJsonObject { put("name", name) }).obj("skill") }
                ?.let { detail = it }
        }
    }
    val viewingDetail = selectedName != null
    PlatformBackHandler(enabled = viewingDetail) {
        selectedName = null
        detail = null
    }

    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = {
                if (viewingDetail) {
                    selectedName = null
                    detail = null
                } else onBack()
            }) { Text("‹", style = MaterialTheme.typography.headlineSmall) }
            Text(stringResource(Res.string.skills_title), Modifier.weight(1f), style = MaterialTheme.typography.titleLarge)
            if (!viewingDetail) {
                TextButton(onClick = { selectedName = null; detail = null; showEditor = true }) { Text("＋ ${stringResource(Res.string.skills_new)}") }
                TextButton(onClick = { showImport = true }) { Text(stringResource(Res.string.skills_import)) }
            }
        }
        actionError?.let { Text(stringResource(Res.string.skills_error, it), color = MaterialTheme.colorScheme.error, modifier = Modifier.padding(horizontal = 12.dp)) }
        if (!viewingDetail) {
            OutlinedTextField(search, { search = it }, Modifier.fillMaxWidth().padding(horizontal = 12.dp), placeholder = { Text(stringResource(Res.string.skills_search)) }, singleLine = true)
            Row(Modifier.fillMaxWidth().padding(12.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                SkillFilter.entries.forEach { candidate -> FilterChip(filter == candidate, { filter = candidate }, label = { Text(candidate.label()) }) }
            }
            HorizontalDivider()
        }
        if (viewingDetail) {
            if (detail != null) {
                SkillDetail(detail!!, bots = state.bots, modifier = Modifier.fillMaxSize(), onEdit = { showEditor = true }, onDelete = {
                    detail?.str("name")?.let { name ->
                        scope.launch {
                            skillRequest({ actionError = it }) {
                                repository.call("skill.delete", buildJsonObject { put("name", name) })
                                repository.call("skill.list")
                            }?.let { selectedName = null; detail = null }
                        }
                    }
                }, onPublish = {
                    detail?.str("name")?.let { name -> scope.launch {
                        skillRequest({ actionError = it }) { publishSkillAndRefresh(repository, name) }
                            ?.let { detail = it }
                    } }
                }, onToggleBot = { botId, enabled ->
                    detail?.str("name")?.let { name -> scope.launch {
                        skillRequest({ actionError = it }) {
                            repository.call("skill.set_enabled", buildJsonObject { put("name", name); put("enabled", enabled); put("bot_id", botId) })
                            repository.call("skill.get", buildJsonObject { put("name", name) })
                        }?.let { detail = it.obj("skill") }
                    } }
                })
            } else {
                Text(stringResource(Res.string.skills_loading), Modifier.padding(20.dp))
            }
        } else {
            LazyColumn(Modifier.fillMaxSize().padding(10.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                items(skills, key = { it.str("name") ?: it.hashCode() }) { skill ->
                    SkillRow(skill, selectedName == skill.str("name"), onClick = { detail = null; selectedName = skill.str("name") }, onToggle = { enabled ->
                        skill.str("name")?.let { name -> scope.launch {
                            skillRequest({ actionError = it }) {
                                repository.call("skill.set_enabled", buildJsonObject { put("name", name); put("enabled", enabled) })
                                repository.call("skill.list")
                            }
                        } }
                    })
                }
                if (skills.isEmpty()) item { Text(stringResource(Res.string.skills_empty), Modifier.padding(20.dp)) }
            }
        }
    }
    if (showEditor) SkillEditor(detail, onDismiss = { showEditor = false }, onSaved = { refreshed ->
        showEditor = false
        if (selectedName != null) detail = refreshed
    }, onError = { actionError = it }, repository = repository)
    if (showImport) SkillImportDialog(onDismiss = { showImport = false; scope.launch { skillRequest({ actionError = it }) { repository.call("skill.list") } } }, onError = { actionError = it }, repository = repository)
}

@Composable
private fun SkillRow(skill: JsonObject, selected: Boolean, onClick: () -> Unit, onToggle: (Boolean) -> Unit) {
    Card(onClick = onClick, modifier = Modifier.fillMaxWidth()) {
        Row(Modifier.fillMaxWidth().padding(10.dp), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text(skill.str("name") ?: "", style = MaterialTheme.typography.titleSmall)
                Text(skill.str("description") ?: "", maxLines = 2, style = MaterialTheme.typography.bodySmall)
                Text(sourceLabel(skill.str("source")), style = MaterialTheme.typography.labelSmall)
            }
            Switch(checked = skill.boolean("enabled") == true, onCheckedChange = onToggle)
        }
    }
}

@Composable
private fun SkillDetail(skill: JsonObject, bots: List<JsonObject>, modifier: Modifier, onEdit: () -> Unit, onDelete: () -> Unit, onPublish: () -> Unit, onToggleBot: (String, Boolean) -> Unit) {
    val disabledBots = skill.arr("disabled_bot_ids").map { it.toString().trim('"') }.toSet()
    val files = skill.arr("files")
    val invocations = skill.obj("invocations_7d")
    val source = skill.str("source")
    val botNames = bots.associate { it.str("id") to botDisplayName(it) }
    Column(modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(12.dp)) {
        Text(skill.str("name") ?: "", style = MaterialTheme.typography.titleLarge)
        Text(skill.str("description") ?: "", style = MaterialTheme.typography.bodyMedium)
        Text(sourceLabel(source), style = MaterialTheme.typography.labelSmall)
        Text("${stringResource(Res.string.skills_path)}: ${skill.str("path")}", style = MaterialTheme.typography.labelSmall)
        Text("${stringResource(Res.string.skills_invocations)}: ${invocations.long("total")}", style = MaterialTheme.typography.labelSmall)
        if (files.isEmpty()) {
            Text(stringResource(Res.string.skills_no_files), style = MaterialTheme.typography.labelSmall)
        } else {
            files.forEach { file -> Text("• ${file.toString().trim('"')}", style = MaterialTheme.typography.labelSmall) }
        }
        invocations.arr("by_bot").forEach { item ->
            val stat = item as? JsonObject ?: return@forEach
            val botId = stat.str("bot_id")
            Text("${botNames[botId].orEmpty().ifBlank { botId }}: ${stat.long("count")}", style = MaterialTheme.typography.labelSmall)
        }
        Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            if (source != "builtin") {
                TextButton(onClick = onEdit) { Text(stringResource(Res.string.skills_edit)) }
                TextButton(onClick = onDelete) { Text(stringResource(Res.string.skills_delete), color = MaterialTheme.colorScheme.error) }
            }
            if (skillCanPublish(skill.str("source"))) TextButton(onClick = onPublish) { Text(stringResource(Res.string.skills_publish)) }
        }
        HorizontalDivider()
        if (bots.isNotEmpty()) {
            Text(stringResource(Res.string.skills_bot_access), style = MaterialTheme.typography.titleSmall, modifier = Modifier.padding(top = 10.dp))
            if (skillBotAccessAvailable(source)) {
                bots.forEach { bot ->
                    val botId = bot.str("id")
                    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                        Text(botDisplayName(bot), Modifier.weight(1f), style = MaterialTheme.typography.bodySmall)
                        Switch(checked = botId !in disabledBots, onCheckedChange = { onToggleBot(botId, it) })
                    }
                }
            } else {
                Text(stringResource(Res.string.skills_draft_bot_access_hint), style = MaterialTheme.typography.bodySmall)
            }
        }
        Text(skill.str("content") ?: stringResource(Res.string.skills_preview), Modifier.padding(top = 10.dp), style = MaterialTheme.typography.bodySmall)
    }
}

internal fun skillCanPublish(source: String?): Boolean = source == "draft"

internal fun skillBotAccessAvailable(source: String): Boolean = source != "draft"

internal fun botDisplayName(bot: JsonObject): String =
    bot.str("label").ifBlank { bot.str("name") }.ifBlank { bot.str("id") }

@Composable
private fun sourceLabel(source: String): String = when (source) {
    "builtin" -> stringResource(Res.string.skills_builtin)
    "user" -> stringResource(Res.string.skills_mine)
    "imported" -> stringResource(Res.string.skills_imported)
    "draft" -> stringResource(Res.string.skills_draft)
    else -> stringResource(Res.string.skills_source_unknown)
}

@Composable
private fun SkillEditor(skill: JsonObject?, onDismiss: () -> Unit, onSaved: (JsonObject) -> Unit, onError: (String) -> Unit, repository: MobileRepository) {
    var name by remember(skill) { mutableStateOf(skill?.str("name") ?: "") }
    var content by remember(skill) { mutableStateOf(skill?.str("content") ?: "") }
    var description by remember(skill) { mutableStateOf(skill?.str("description") ?: "") }
    val scope = rememberCoroutineScope()
    AlertDialog(onDismissRequest = onDismiss, title = { Text(if (skill == null) stringResource(Res.string.skills_new) else stringResource(Res.string.skills_edit)) }, text = {
        Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlinedTextField(name, { name = it }, enabled = skill == null, label = { Text(stringResource(Res.string.skills_name)) }, singleLine = true)
            OutlinedTextField(description, { description = it }, label = { Text(stringResource(Res.string.skills_description)) }, singleLine = false)
            OutlinedTextField(content, { content = it }, label = { Text(stringResource(Res.string.skills_content)) }, minLines = 8)
        }
    }, confirmButton = { Button(enabled = name.isNotBlank() && content.isNotBlank(), onClick = {
        scope.launch {
            skillRequest({ onError(it) }) {
                saveSkillAndRefresh(repository, skill?.str("name"), name, description, content)
            }?.let(onSaved)
        }
    }) { Text(stringResource(Res.string.skills_save)) } }, dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(Res.string.common_close)) } })
}

@Composable
private fun SkillImportDialog(onDismiss: () -> Unit, onError: (String) -> Unit, repository: MobileRepository) {
    var url by remember { mutableStateOf("") }
    var file by remember { mutableStateOf<PickedFile?>(null) }
    val scope = rememberCoroutineScope()
    AlertDialog(onDismissRequest = onDismiss, title = { Text(stringResource(Res.string.skills_import)) }, text = {
        Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlinedTextField(url, { url = it }, label = { Text(stringResource(Res.string.skills_git_url)) }, singleLine = true)
            TextButton(onClick = { scope.launch {
                file = try {
                    platformFilePicker().pickFile()
                } catch (cancelled: CancellationException) {
                    throw cancelled
                } catch (_: Throwable) {
                    null
                }
            } }) { Text(stringResource(Res.string.skills_import)) }
            file?.let { Text("${stringResource(Res.string.skills_files)}: ${it.name}", style = MaterialTheme.typography.labelSmall) }
        }
    }, confirmButton = { Button(enabled = url.isNotBlank() || file != null, onClick = {
        scope.launch {
            skillRequest({ onError(it) }) {
                if (url.isNotBlank()) repository.call("skill.import", buildJsonObject { put("source", buildJsonObject { put("kind", "git"); put("url", url.trim()) }) })
                else file?.let { picked ->
                    val upload = repository.uploadFile(picked)
                    val uploadId = upload.str("upload_id") ?: error("upload id missing")
                    repository.call("skill.import", buildJsonObject { put("source", buildJsonObject { put("kind", "upload"); put("upload_id", uploadId) }) })
                }
            }?.let { onDismiss() }
        }
    }) { Text(stringResource(Res.string.skills_import)) } }, dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(Res.string.common_close)) } })
}

private fun skillContent(name: String, description: String, content: String): String {
    if (description.isBlank() || content.trimStart().startsWith("---")) return content
    val safeDescription = description.replace('\n', ' ').replace('\r', ' ')
    return "---\nname: $name\ndescription: $safeDescription\n---\n$content"
}

@Composable
private fun SkillFilter.label(): String = when (this) {
    SkillFilter.ALL -> stringResource(Res.string.skills_all)
    SkillFilter.BUILTIN -> stringResource(Res.string.skills_builtin)
    SkillFilter.MINE -> stringResource(Res.string.skills_mine)
    SkillFilter.IMPORTED -> stringResource(Res.string.skills_imported)
    SkillFilter.DRAFT -> stringResource(Res.string.skills_draft)
}
