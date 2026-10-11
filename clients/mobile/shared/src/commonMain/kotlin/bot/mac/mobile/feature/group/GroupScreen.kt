package bot.mac.mobile.feature.group

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.material3.Button
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.design.FontScaleToolbar
import bot.mac.mobile.core.protocol.arr
import bot.mac.mobile.core.protocol.boolean
import bot.mac.mobile.core.protocol.obj
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.core.network.NetworkError
import bot.mac.mobile.feature.chat.ChatScreen
import bot.mac.mobile.feature.chat.StatusLabel
import bot.mac.mobile.core.ui.MarkdownText
import bot.mac.mobile.resources.*
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.jetbrains.compose.resources.stringResource

@Composable
fun GroupScreen(
    repository: MobileRepository,
    projectId: String,
    onOpenTrace: (String) -> Unit = {},
    onBack: () -> Unit = {},
    onOpenProject: (String) -> Unit = {},
    onOpenArtifact: (artifactId: String, pathOrUrl: String, projectId: String?) -> Unit = { _, _, _ -> },
    onOpenHome: (String) -> Unit = {},
    onCopyHome: (String) -> Unit = {},
    onOpenHistory: (String) -> Unit = {},
    onOpenScreen: (String, String?) -> Unit = { _, _ -> },
    onOpenChat: (String) -> Unit = {},
    onLoopAction: (String, String) -> Unit = { _, _ -> },
    onTakeover: (String) -> Unit = {},
    onOpenGroupInfo: (String) -> Unit = {},
    editAnnouncementOnOpen: Boolean = false,
) {
    val state by repository.state.collectAsState()
    val project = state.projects.firstOrNull { it.str("id") == projectId }
    val chatId = project?.str("chat_id")?.takeIf { it.isNotBlank() }
    val announcement = state.announcements[projectId]
    var showAnnouncement by remember { mutableStateOf(false) }
    var changeText by remember { mutableStateOf("") }
    var showChangeInput by remember { mutableStateOf(false) }
    var changeProjectId by remember { mutableStateOf<String?>(null) }
    var showEdit by remember { mutableStateOf(false) }
    var showAnnouncementEditor by remember { mutableStateOf(false) }
    var announcementText by remember(projectId) { mutableStateOf(announcement?.str("text").orEmpty()) }
    var announcementQuoteToAppend by remember(projectId) { mutableStateOf<String?>(null) }
    var announcementBaseVersion by remember(projectId) { mutableStateOf(announcement?.get("version")?.toString()?.trim('"')?.toLongOrNull() ?: 0L) }
    var announcementConflict by remember { mutableStateOf<JsonObject?>(null) }
    var announcementSaving by remember { mutableStateOf(false) }
    var editName by remember(projectId) { mutableStateOf(project?.str("name").orEmpty()) }
    var error by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()

    LaunchedEffect(announcement?.str("version"), showAnnouncementEditor) {
        if (!showAnnouncementEditor) {
            announcementText = announcement?.str("text").orEmpty()
        } else {
            announcementQuoteToAppend?.let { quote ->
                val current = announcement?.str("text").orEmpty()
                announcementText = listOf(current, quote).filter(String::isNotBlank).joinToString("\n\n")
                announcementQuoteToAppend = null
            }
        }
    }

    if (showAnnouncementEditor) {
        androidx.compose.ui.window.Dialog(
            onDismissRequest = { showAnnouncementEditor = false; announcementConflict = null },
            properties = androidx.compose.ui.window.DialogProperties(usePlatformDefaultWidth = false),
        ) {
            Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
                Column(Modifier.fillMaxSize().imePadding().padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    FontScaleToolbar {
                        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            Button(onClick = { showAnnouncementEditor = false; announcementConflict = null }) { Text(stringResource(Res.string.feature_back)) }
                            Text(stringResource(Res.string.feature_announcement_edit), Modifier.weight(1f), style = MaterialTheme.typography.titleLarge)
                            Button(enabled = !announcementSaving && announcementText.length <= 8000, onClick = {
                            announcementSaving = true
                            scope.launch {
                                runCatching {
                                    repository.call("announcement.update", buildJsonObject {
                                        put("project_id", projectId); put("text", announcementText); put("base_version", announcementBaseVersion)
                                    })
                                }.onSuccess {
                                    announcementConflict = null
                                    showAnnouncementEditor = false
                                }.onFailure { failure ->
                                    val network = failure as? NetworkError
                                    val latest = network?.details?.obj("details")?.obj("announcement")
                                    if (network?.code == "conflict" && latest != null) {
                                        announcementConflict = latest
                                        announcementBaseVersion = latest["version"]?.toString()?.trim('"')?.toLongOrNull() ?: announcementBaseVersion
                                    }
                                    else error = failure.message
                                }
                                announcementSaving = false
                            }
                            }) { Text(stringResource(Res.string.common_save)) }
                        }
                    }
                    Text(stringResource(Res.string.feature_announcement_edit_hint), style = MaterialTheme.typography.bodySmall)
                    Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        listOf("#", "**", "• ", "1. ", "`", "[ ]()").forEach { syntax ->
                            FilterChip(selected = false, onClick = { announcementText += syntax }, label = { Text(syntax) })
                        }
                    }
                    OutlinedTextField(
                        value = announcementText, onValueChange = { announcementText = it.take(8000) },
                        modifier = Modifier.fillMaxWidth().weight(1f), minLines = 8,
                        label = { Text(stringResource(Res.string.feature_announcement_body)) },
                        supportingText = { Text(stringResource(Res.string.feature_announcement_version, announcement?.str("version").orEmpty(), announcementText.length)) },
                    )
                    announcementConflict?.let { latest ->
                        Surface(color = MaterialTheme.colorScheme.errorContainer, shape = MaterialTheme.shapes.medium) {
                            Column(Modifier.fillMaxWidth().padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                Text(stringResource(Res.string.feature_announcement_conflict), color = MaterialTheme.colorScheme.onErrorContainer)
                                Text(latest.str("text"), maxLines = 5, color = MaterialTheme.colorScheme.onErrorContainer)
                                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                    Button(onClick = { announcementText = latest.str("text"); announcementBaseVersion = latest["version"]?.toString()?.trim('"')?.toLongOrNull() ?: announcementBaseVersion; announcementConflict = null }) { Text(stringResource(Res.string.feature_announcement_load_latest)) }
                                    Button(onClick = { announcementBaseVersion = latest["version"]?.toString()?.trim('"')?.toLongOrNull() ?: announcementBaseVersion; announcementConflict = null }) { Text(stringResource(Res.string.feature_announcement_keep_draft)) }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    LaunchedEffect(project?.str("name")) {
        if (!showEdit) {
            editName = project?.str("name").orEmpty()
        }
    }

    LaunchedEffect(projectId) {
        runCatching {
            repository.call("project.get", buildJsonObject { put("project_id", projectId) })
        }.onFailure { error = it.message }
    }
    LaunchedEffect(projectId, editAnnouncementOnOpen) {
        if (editAnnouncementOnOpen) showAnnouncementEditor = true
    }
    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        FontScaleToolbar {
            Row(Modifier.fillMaxWidth().padding(12.dp)) {
                Button(onClick = onBack) { Text(stringResource(Res.string.feature_back)) }
                Column(Modifier.weight(1f).padding(start = 12.dp)) {
                    Text(project?.str("name")?.takeIf { it.isNotBlank() } ?: stringResource(Res.string.feature_group), style = MaterialTheme.typography.titleLarge, maxLines = 1, overflow = TextOverflow.MiddleEllipsis)
                    project?.str("status")?.takeIf { it.isNotBlank() }?.let { StatusLabel(it) }
                }
                Button(onClick = { showAnnouncement = !showAnnouncement }) { Text(stringResource(Res.string.feature_announcement)) }
                Button(onClick = { onOpenGroupInfo(projectId) }) { Text(stringResource(Res.string.feature_group_info)) }
                Button(onClick = { announcementText = announcement?.str("text").orEmpty(); announcementBaseVersion = announcement?.get("version")?.toString()?.trim('"')?.toLongOrNull() ?: 0L; showAnnouncementEditor = true }) { Text(stringResource(Res.string.feature_edit)) }
                Button(onClick = { showEdit = !showEdit }) { Text(stringResource(Res.string.feature_edit_group)) }
            }
        }
        if (showEdit) {
            Column(Modifier.fillMaxWidth().heightIn(max = 320.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(editName, { editName = it.take(32) }, Modifier.fillMaxWidth().padding(horizontal = 12.dp), label = { Text(stringResource(Res.string.feature_group_name)) }, supportingText = { Text(stringResource(Res.string.feature_name_max_length, 32)) })
                Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 12.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Button(onClick = {
                        scope.launch { runCatching { repository.call("project.update", buildJsonObject { put("project_id", projectId); put("patch", buildJsonObject { put("name", editName.trim()) }) }) }.onFailure { error = it.message } }
                        showEdit = false
                    }) { Text(stringResource(Res.string.feature_save_group)) }
                }
                Text(stringResource(Res.string.feature_add_member), Modifier.padding(horizontal = 12.dp), style = MaterialTheme.typography.titleSmall)
                val memberIds = project?.arr("members")?.mapNotNull { (it as? JsonObject)?.str("bot_id") }.orEmpty()
                state.bots.filter { !it.boolean("is_main") }.forEach { bot ->
                    val id = bot.str("id")
                    val inProject = id in memberIds
                    Button(onClick = {
                        scope.launch { runCatching { repository.call(if (inProject) "project.remove_member" else "project.add_member", buildJsonObject { put("project_id", projectId); put("bot_id", id) }) }.onFailure { error = it.message } }
                    }) { Text(if (inProject) "${bot.str("name")} · ${stringResource(Res.string.feature_remove_member)}" else "＋ ${bot.str("name")}") }
                }
            }
        }
        error?.let { Text(it, Modifier.padding(horizontal = 12.dp), color = MaterialTheme.colorScheme.error) }
        val announcementMembers = announcement?.arr("members").orEmpty()
        (if (announcementMembers.isNotEmpty()) announcementMembers else project?.arr("members").orEmpty()).let { members ->
            Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 12.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                members.forEach { member ->
                    val m = member as? JsonObject
                    val botId = m?.str("bot_id").orEmpty()
                    val botName = state.bots.firstOrNull { it.str("id") == botId }?.str("name").orEmpty().ifBlank { botId }
                    Column(Modifier.widthIn(min = 150.dp)) {
                        Text("● $botName", style = MaterialTheme.typography.labelMedium)
                        m?.let { member ->
                            member.str("role_note").takeIf { it.isNotBlank() }?.let { Text(it, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall) }
                            member.str("state").takeIf { it.isNotBlank() }?.let { StatusLabel(it) }
                            val assignmentId = member.str("current_assignment_id")
                            val assignment = state.assignments.firstOrNull { it.str("id") == assignmentId }
                            if (assignmentId.isNotBlank()) {
                                Button(onClick = { onOpenTrace(assignmentId) }) {
                                    Text(assignment?.str("title").orEmpty().ifBlank { stringResource(Res.string.feature_detail) })
                                }
                            }
                        }
                    }
                }
            }
        }
        if (showAnnouncement) AnnouncementPanel(announcement, project, onOpenArtifact, onOpenHome, onCopyHome)
        HorizontalDivider(Modifier.padding(top = 8.dp))
        Box(Modifier.weight(1f).fillMaxWidth()) {
            chatId?.let { id ->
                ChatScreen(
                    repository = repository,
                    chatId = id,
                    onOpenTrace = { assignment, _ -> assignment?.let(onOpenTrace) },
                    onOpenProject = onOpenProject,
                    onOpenArtifact = onOpenArtifact,
                    onOpenHistory = onOpenHistory,
                    artifactProjectId = projectId,
                    onBack = {},
                    onProjectAction = { targetProjectId, action ->
                        if (action == "request_changes") {
                            changeProjectId = targetProjectId
                            showChangeInput = true
                        } else {
                            scope.launch {
                                runCatching { repository.call("project.$action", buildJsonObject { put("project_id", targetProjectId) }) }
                                    .onFailure { error = it.message }
                            }
                        }
                    },
                    onOpenScreen = onOpenScreen,
                    onOpenChat = onOpenChat,
                    onQuoteAnnouncement = { quote ->
                        if (showAnnouncementEditor) {
                            announcementText = listOf(announcementText, quote).filter(String::isNotBlank).joinToString("\n\n")
                        } else {
                            announcementQuoteToAppend = quote
                            showAnnouncementEditor = true
                        }
                    },
                    onLoopAction = onLoopAction,
                    onTakeover = onTakeover,
                    showHeader = false,
                )
            } ?: Text(stringResource(Res.string.feature_group_empty), Modifier.padding(24.dp))
        }
        if (showChangeInput) {
            OutlinedTextField(value = changeText, onValueChange = { changeText = it }, modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp), label = { Text(stringResource(Res.string.feature_change_hint)) })
        }
        Row(Modifier.fillMaxWidth().padding(10.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            if (project?.str("status") == "review" || project?.str("status") == "active") {
                Button(onClick = { scope.launch { runCatching { repository.call("project.confirm_done", buildJsonObject { put("project_id", projectId) }) }.onFailure { error = it.message } } }) { Text(stringResource(Res.string.feature_confirm_done)) }
            }
            if (project?.str("status") == "review" || project?.str("status") == "active") {
                Button(onClick = {
                    if (!showChangeInput) {
                        changeProjectId = projectId
                        showChangeInput = true
                    }
                    else {
                        val text = changeText.trim()
                        if (text.isNotEmpty()) {
                            val targetProject = changeProjectId ?: projectId
                            scope.launch { runCatching { repository.call("project.request_changes", buildJsonObject { put("project_id", targetProject); put("text", text) }) }.onFailure { error = it.message } }
                        }
                        changeText = ""
                        changeProjectId = null
                        showChangeInput = false
                    }
                }) { Text(if (showChangeInput) stringResource(Res.string.feature_submit_changes) else stringResource(Res.string.feature_request_changes)) }
            }
            if (project?.str("status") == "done") {
                Button(onClick = { scope.launch { runCatching { repository.call("project.archive", buildJsonObject { put("project_id", projectId) }) }.onFailure { error = it.message } } }) { Text(stringResource(Res.string.feature_archive)) }
            }
            if (project?.str("status") == "archived") {
                Button(onClick = { scope.launch { runCatching { repository.call("project.reopen", buildJsonObject { put("project_id", projectId) }) }.onFailure { error = it.message } } }) { Text(stringResource(Res.string.feature_reopen)) }
            }
        }
    }
}

@Composable
fun GroupCreateScreen(
    repository: MobileRepository,
    onCreated: (projectId: String) -> Unit = {},
    onBack: () -> Unit = {},
) {
    val state by repository.state.collectAsState()
    var name by remember { mutableStateOf("") }
    var goal by remember { mutableStateOf("") }
    var selected by remember { mutableStateOf(emptySet<String>()) }
    var error by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    val createFailed = stringResource(Res.string.feature_create_failed)
    Column(Modifier.fillMaxSize().imePadding().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        FontScaleToolbar { Row(Modifier.fillMaxWidth()) { Button(onClick = onBack) { Text(stringResource(Res.string.feature_back)) }; Text(stringResource(Res.string.feature_create_group), Modifier.padding(start = 12.dp), style = MaterialTheme.typography.titleLarge) } }
        OutlinedTextField(name, { name = it.take(32) }, Modifier.fillMaxWidth(), label = { Text(stringResource(Res.string.feature_group_name)) }, supportingText = { Text(stringResource(Res.string.feature_name_max_length, 32)) })
        OutlinedTextField(goal, { goal = it }, Modifier.fillMaxWidth(), minLines = 3, label = { Text(stringResource(Res.string.feature_announcement_body)) })
        Text(stringResource(Res.string.feature_group_members), style = MaterialTheme.typography.titleSmall)
        state.bots.filter { !it.boolean("is_main") && !it.boolean("hidden") }.forEach { bot ->
            val id = bot.str("id").takeIf { it.isNotBlank() } ?: return@forEach
            val isSelected = selected.contains(id)
            FilterChip(
                selected = isSelected,
                enabled = isSelected || selected.size < 6,
                onClick = { selected = if (isSelected) selected - id else selected + id },
                label = { Text(bot.str("name").ifBlank { id }) },
            )
        }
        error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        Button(enabled = name.isNotBlank() && selected.size in 1..6, onClick = {
            scope.launch {
                runCatching {
                    repository.call("project.create", buildJsonObject {
                        put("name", name.trim())
                        if (goal.isNotBlank()) put("announcement", goal.trim())
                        put("member_bot_ids", buildJsonArray { selected.forEach { add(JsonPrimitive(it)) } })
                    })
                }.onSuccess { result ->
                    val id = result.str("id").takeIf { it.isNotBlank() } ?: result.obj("project").str("id").takeIf { it.isNotBlank() }
                    if (id == null) error = createFailed else onCreated(id)
                }.onFailure { error = it.message ?: createFailed }
            }
        }, modifier = Modifier.fillMaxWidth()) { Text(stringResource(Res.string.feature_create)) }
    }
}

@Composable
private fun AnnouncementPanel(
    announcement: JsonObject?,
    project: JsonObject?,
    onOpenArtifact: (artifactId: String, pathOrUrl: String, projectId: String?) -> Unit,
    onOpenHome: (String) -> Unit,
    onCopyHome: (String) -> Unit,
) {
    Surface(Modifier.fillMaxWidth().padding(12.dp), color = MaterialTheme.colorScheme.secondaryContainer) {
        Column(Modifier.heightIn(max = 300.dp).verticalScroll(rememberScrollState()).padding(12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text(stringResource(Res.string.feature_announcement_title, project?.str("name") ?: ""), style = MaterialTheme.typography.titleMedium)
            project?.str("status")?.takeIf { it.isNotBlank() }?.let { StatusLabel(it) }
            if (announcement?.str("text").orEmpty().isNotBlank()) MarkdownText(announcement?.str("text").orEmpty(), Modifier.fillMaxWidth())
            else Text(stringResource(Res.string.feature_announcement_empty), color = MaterialTheme.colorScheme.onSurfaceVariant)
            announcement?.str("text_updated_at")?.takeIf { it.isNotBlank() }?.let { Text(stringResource(Res.string.feature_announcement_updated, it), style = MaterialTheme.typography.bodySmall) }
            val home = project?.str("home_path").orEmpty()
            Text(stringResource(Res.string.feature_home_value, home))
            if (home.isNotBlank()) {
                Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    Button(onClick = { onOpenHome(home) }) { Text(stringResource(Res.string.feature_home_open)) }
                    Button(onClick = { onCopyHome(home) }) { Text(stringResource(Res.string.feature_home_copy)) }
                }
            }
            announcement?.arr("artifacts")?.forEach { item ->
                val artifact = item as? JsonObject ?: return@forEach
                val artifactId = artifact.str("id")
                val pathOrUrl = artifact.str("path_or_url")
                if (artifactId.isNotBlank() && pathOrUrl.isNotBlank()) {
                    Column {
                        Text(stringResource(Res.string.feature_artifact_author, artifact.str("bot_id").ifBlank { stringResource(Res.string.feature_bot) }), style = MaterialTheme.typography.labelSmall)
                        Button(onClick = { onOpenArtifact(artifactId, pathOrUrl, project?.str("id")) }) {
                            Text("▤ ${artifact.str("title").ifBlank { pathOrUrl }}")
                        }
                    }
                }
            }
        }
    }
}
