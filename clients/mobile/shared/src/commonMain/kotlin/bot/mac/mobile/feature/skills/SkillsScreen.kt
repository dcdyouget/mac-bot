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
import bot.mac.mobile.core.protocol.obj
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.platform.PickedFile
import bot.mac.mobile.core.platform.platformFilePicker
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.resources.*
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.jetbrains.compose.resources.stringResource

private enum class SkillFilter { ALL, BUILTIN, MINE, IMPORTED, DRAFT }

@Composable
fun SkillsScreen(repository: MobileRepository, onBack: () -> Unit) {
    val state by repository.state.collectAsState()
    var selectedName by remember { mutableStateOf<String?>(null) }
    var search by remember { mutableStateOf("") }
    var filter by remember { mutableStateOf(SkillFilter.ALL) }
    var showEditor by remember { mutableStateOf(false) }
    var showImport by remember { mutableStateOf(false) }
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

    LaunchedEffect(Unit) { repository.call("skill.list") }
    LaunchedEffect(selectedName) {
        selectedName?.let { detail = repository.call("skill.get", buildJsonObject { put("name", it) }).obj("skill") }
    }

    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = onBack) { Text("‹", style = MaterialTheme.typography.headlineSmall) }
            Text(stringResource(Res.string.skills_title), Modifier.weight(1f), style = MaterialTheme.typography.titleLarge)
            TextButton(onClick = { selectedName = null; detail = null; showEditor = true }) { Text("＋ ${stringResource(Res.string.skills_new)}") }
            TextButton(onClick = { showImport = true }) { Text(stringResource(Res.string.skills_import)) }
        }
        OutlinedTextField(search, { search = it }, Modifier.fillMaxWidth().padding(horizontal = 12.dp), placeholder = { Text(stringResource(Res.string.skills_search)) }, singleLine = true)
        Row(Modifier.fillMaxWidth().padding(12.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            SkillFilter.entries.forEach { candidate -> FilterChip(filter == candidate, { filter = candidate }, label = { Text(candidate.label()) }) }
        }
        HorizontalDivider()
        Row(Modifier.fillMaxSize()) {
            LazyColumn(Modifier.weight(0.9f).fillMaxSize().padding(10.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                items(skills, key = { it.str("name") ?: it.hashCode() }) { skill ->
                    SkillRow(skill, selectedName == skill.str("name"), onClick = { selectedName = skill.str("name") }, onToggle = { enabled ->
                        skill.str("name")?.let { name -> scope.launch { repository.call("skill.set_enabled", buildJsonObject { put("name", name); put("enabled", enabled) }) } }
                    })
                }
                if (skills.isEmpty()) item { Text(stringResource(Res.string.skills_empty), Modifier.padding(20.dp)) }
            }
            if (detail != null) {
                SkillDetail(detail!!, bots = state.bots, modifier = Modifier.weight(1.1f), onEdit = { showEditor = true }, onDelete = {
                    detail?.str("name")?.let { name ->
                        scope.launch { repository.call("skill.delete", buildJsonObject { put("name", name) }); selectedName = null; detail = null }
                    }
                }, onPublish = {
                    detail?.str("name")?.let { name -> scope.launch { repository.call("skill.publish", buildJsonObject { put("name", name) }) } }
                }, onToggleBot = { botId, enabled ->
                    detail?.str("name")?.let { name -> scope.launch { repository.call("skill.set_enabled", buildJsonObject { put("name", name); put("enabled", enabled); put("bot_id", botId) }) } }
                })
            }
        }
    }
    if (showEditor) SkillEditor(detail, onDismiss = { showEditor = false }, onSaved = { showEditor = false; scope.launch { repository.call("skill.list") } }, repository = repository)
    if (showImport) SkillImportDialog(onDismiss = { showImport = false }, repository = repository)
}

@Composable
private fun SkillRow(skill: JsonObject, selected: Boolean, onClick: () -> Unit, onToggle: (Boolean) -> Unit) {
    Card(onClick = onClick, modifier = Modifier.fillMaxWidth()) {
        Row(Modifier.fillMaxWidth().padding(10.dp), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text(skill.str("name") ?: "", style = MaterialTheme.typography.titleSmall)
                Text(skill.str("description") ?: "", maxLines = 2, style = MaterialTheme.typography.bodySmall)
                Text(skill.str("source") ?: "", style = MaterialTheme.typography.labelSmall)
            }
            Switch(checked = skill.boolean("enabled") == true, onCheckedChange = onToggle)
        }
    }
}

@Composable
private fun SkillDetail(skill: JsonObject, bots: List<JsonObject>, modifier: Modifier, onEdit: () -> Unit, onDelete: () -> Unit, onPublish: () -> Unit, onToggleBot: (String, Boolean) -> Unit) {
    val disabledBots = skill.arr("disabled_bot_ids").map { it.toString().trim('"') }.toSet()
    Column(modifier.fillMaxSize().padding(12.dp)) {
        Text(skill.str("name") ?: "", style = MaterialTheme.typography.titleLarge)
        Text(skill.str("description") ?: "", style = MaterialTheme.typography.bodyMedium)
        Text("${stringResource(Res.string.skills_files)}: ${skill.str("path") ?: ""}", style = MaterialTheme.typography.labelSmall)
        Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            TextButton(onClick = onEdit) { Text(stringResource(Res.string.skills_edit)) }
            TextButton(onClick = onDelete) { Text(stringResource(Res.string.skills_delete), color = MaterialTheme.colorScheme.error) }
            if (skillCanPublish(skill.str("source"))) TextButton(onClick = onPublish) { Text(stringResource(Res.string.skills_publish)) }
        }
        HorizontalDivider()
        if (bots.isNotEmpty()) {
            Text(stringResource(Res.string.skills_bot_access), style = MaterialTheme.typography.titleSmall, modifier = Modifier.padding(top = 10.dp))
            bots.forEach { bot ->
                val botId = bot.str("id")
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                    Text(bot.str("label") ?: bot.str("name") ?: botId, Modifier.weight(1f), style = MaterialTheme.typography.bodySmall)
                    Switch(checked = botId !in disabledBots, onCheckedChange = { onToggleBot(botId, it) })
                }
            }
        }
        Text(skill.str("content") ?: stringResource(Res.string.skills_preview), Modifier.padding(top = 10.dp), style = MaterialTheme.typography.bodySmall)
    }
}

internal fun skillCanPublish(source: String?): Boolean = source == "draft"

@Composable
private fun SkillEditor(skill: JsonObject?, onDismiss: () -> Unit, onSaved: () -> Unit, repository: MobileRepository) {
    var name by remember(skill) { mutableStateOf(skill?.str("name") ?: "") }
    var content by remember(skill) { mutableStateOf(skill?.str("content") ?: "") }
    var description by remember(skill) { mutableStateOf(skill?.str("description") ?: "") }
    val scope = rememberCoroutineScope()
    AlertDialog(onDismissRequest = onDismiss, title = { Text(if (skill == null) stringResource(Res.string.skills_new) else stringResource(Res.string.skills_edit)) }, text = {
        Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlinedTextField(name, { name = it }, label = { Text(stringResource(Res.string.skills_name)) }, singleLine = true)
            OutlinedTextField(description, { description = it }, label = { Text(stringResource(Res.string.skills_description)) }, singleLine = false)
            OutlinedTextField(content, { content = it }, label = { Text(stringResource(Res.string.skills_content)) }, minLines = 8)
        }
    }, confirmButton = { Button(enabled = name.isNotBlank() && content.isNotBlank(), onClick = {
        scope.launch {
            repository.call(if (skill == null) "skill.create" else "skill.update", buildJsonObject { put("name", name.trim()); put("content", content) })
            onSaved()
        }
    }) { Text(stringResource(Res.string.skills_save)) } }, dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(Res.string.common_close)) } })
}

@Composable
private fun SkillImportDialog(onDismiss: () -> Unit, repository: MobileRepository) {
    var url by remember { mutableStateOf("") }
    var file by remember { mutableStateOf<PickedFile?>(null) }
    val scope = rememberCoroutineScope()
    AlertDialog(onDismissRequest = onDismiss, title = { Text(stringResource(Res.string.skills_import)) }, text = {
        Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlinedTextField(url, { url = it }, label = { Text(stringResource(Res.string.skills_git_url)) }, singleLine = true)
            TextButton(onClick = { scope.launch { file = try { platformFilePicker().pickFile() } catch (_: Throwable) { null } } }) { Text(stringResource(Res.string.skills_import)) }
            file?.let { Text("${stringResource(Res.string.skills_files)}: ${it.name}", style = MaterialTheme.typography.labelSmall) }
        }
    }, confirmButton = { Button(enabled = url.isNotBlank() || file != null, onClick = {
        scope.launch {
            if (url.isNotBlank()) repository.call("skill.import", buildJsonObject { put("source", buildJsonObject { put("kind", "git"); put("url", url.trim()) }) })
            else file?.let { picked ->
                val upload = repository.uploadFile(picked)
                upload.str("upload_id")?.let { id -> repository.call("skill.import", buildJsonObject { put("source", buildJsonObject { put("kind", "upload"); put("upload_id", id) }) }) }
            }
            onDismiss()
        }
    }) { Text(stringResource(Res.string.skills_import)) } }, dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(Res.string.common_close)) } })
}

@Composable
private fun SkillFilter.label(): String = when (this) {
    SkillFilter.ALL -> stringResource(Res.string.skills_all)
    SkillFilter.BUILTIN -> stringResource(Res.string.skills_builtin)
    SkillFilter.MINE -> stringResource(Res.string.skills_mine)
    SkillFilter.IMPORTED -> stringResource(Res.string.skills_imported)
    SkillFilter.DRAFT -> stringResource(Res.string.skills_draft)
}
