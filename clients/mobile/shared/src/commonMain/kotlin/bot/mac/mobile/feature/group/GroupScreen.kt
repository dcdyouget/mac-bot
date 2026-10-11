package bot.mac.mobile.feature.group

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.layout.*
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.IconButton
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
import androidx.compose.ui.Alignment
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.design.BotAvatar
import bot.mac.mobile.core.design.FontScaleToolbar
import bot.mac.mobile.core.protocol.arr
import bot.mac.mobile.core.protocol.boolean
import bot.mac.mobile.core.protocol.obj
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.core.ui.LucideIcon
import bot.mac.mobile.core.ui.LucideIconKind
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
    initialMessageId: String? = null,
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
    var showGroupActions by remember { mutableStateOf(false) }
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
                            Button(onClick = { showAnnouncementEditor = false; announcementConflict = null }) { Text(stringResource(Res.string.common_back)) }
                            Text(stringResource(Res.string.group_announcement_edit_title), Modifier.weight(1f), style = MaterialTheme.typography.titleLarge)
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
    val announcementMembers = announcement?.arr("members").orEmpty()
    val projectMembers = project?.arr("members").orEmpty()
    val chatMemberIds = state.chats.firstOrNull { it.str("id") == chatId }?.arr("member_bot_ids").orEmpty()
        .mapNotNull { (it as? JsonPrimitive)?.content?.takeIf(String::isNotBlank) }
    val members = when {
        announcementMembers.isNotEmpty() -> announcementMembers
        projectMembers.isNotEmpty() -> projectMembers
        else -> chatMemberIds.map { botId ->
            val botStatus = state.bots.firstOrNull { it.str("id") == botId }?.obj("status")?.str("summary").orEmpty()
            buildJsonObject { put("bot_id", botId); put("state", botStatus.ifBlank { "idle" }) }
        }
    }
    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        FontScaleToolbar {
            Row(
                Modifier.fillMaxWidth().height(64.dp).padding(horizontal = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                IconButton(onClick = onBack, modifier = Modifier.size(40.dp)) {
                    LucideIcon(LucideIconKind.ArrowLeft, Modifier.size(20.dp), contentDescription = stringResource(Res.string.common_back))
                }
                Box(Modifier.padding(start = 4.dp, end = 10.dp)) { GroupAvatar(repository, members, state.bots) }
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.Center) {
                    Text(
                        project?.str("name")?.takeIf { it.isNotBlank() } ?: stringResource(Res.string.search_filter_groups),
                        style = MaterialTheme.typography.titleMedium.copy(fontWeight = FontWeight.SemiBold),
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                        val projectStatus = when (project?.str("status")) {
                            "active" -> stringResource(Res.string.project_status_active)
                            "review" -> stringResource(Res.string.project_status_review)
                            "done" -> stringResource(Res.string.project_status_done)
                            "archived" -> stringResource(Res.string.project_status_archived)
                            else -> null
                        }
                        Text(
                            listOfNotNull(projectStatus, stringResource(Res.string.chat_group_subtitle, members.size)).joinToString(" · "),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                }
                IconButton(onClick = { showAnnouncement = !showAnnouncement }, modifier = Modifier.size(40.dp)) {
                    LucideIcon(LucideIconKind.MessageCircle, Modifier.size(20.dp), tint = MaterialTheme.colorScheme.primary, contentDescription = stringResource(Res.string.group_announcement))
                }
                Box {
                    IconButton(onClick = { showGroupActions = true }, modifier = Modifier.size(40.dp)) {
                        LucideIcon(LucideIconKind.Ellipsis, Modifier.size(20.dp), contentDescription = stringResource(Res.string.group_info))
                    }
                    DropdownMenu(expanded = showGroupActions, onDismissRequest = { showGroupActions = false }) {
                        DropdownMenuItem(text = { Text(stringResource(Res.string.group_info)) }, onClick = { showGroupActions = false; onOpenGroupInfo(projectId) })
                        DropdownMenuItem(text = { Text(stringResource(Res.string.group_announcement_edit)) }, onClick = {
                            showGroupActions = false
                            announcementText = announcement?.str("text").orEmpty()
                            announcementBaseVersion = announcement?.get("version")?.toString()?.trim('"')?.toLongOrNull() ?: 0L
                            showAnnouncementEditor = true
                        })
                        DropdownMenuItem(text = { Text(stringResource(Res.string.feature_edit_group)) }, onClick = { showGroupActions = false; showEdit = !showEdit })
                        if (project?.str("status") == "review" || project?.str("status") == "active") {
                            DropdownMenuItem(text = { Text(stringResource(Res.string.common_done)) }, onClick = {
                                showGroupActions = false
                                scope.launch { runCatching { repository.call("project.confirm_done", buildJsonObject { put("project_id", projectId) }) }.onFailure { error = it.message } }
                            })
                            DropdownMenuItem(text = { Text(stringResource(Res.string.block_review_card_request_changes)) }, onClick = {
                                showGroupActions = false
                                changeProjectId = projectId
                                showChangeInput = true
                            })
                        }
                        if (project?.str("status") == "done") {
                            DropdownMenuItem(text = { Text(stringResource(Res.string.sidebar_menu_archive)) }, onClick = {
                                showGroupActions = false
                                scope.launch { runCatching { repository.call("project.archive", buildJsonObject { put("project_id", projectId) }) }.onFailure { error = it.message } }
                            })
                        }
                        if (project?.str("status") == "archived") {
                            DropdownMenuItem(text = { Text(stringResource(Res.string.feature_reopen)) }, onClick = {
                                showGroupActions = false
                                scope.launch { runCatching { repository.call("project.reopen", buildJsonObject { put("project_id", projectId) }) }.onFailure { error = it.message } }
                            })
                        }
                    }
                }
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
        Row(
            Modifier.fillMaxWidth().height(38.dp).horizontalScroll(rememberScrollState()).padding(horizontal = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            members.forEach { rawMember ->
                val member = rawMember as? JsonObject
                val botId = member?.str("bot_id").orEmpty()
                val bot = state.bots.firstOrNull { it.str("id") == botId }
                val botName = bot?.str("name").orEmpty().ifBlank { botId }
                val status = member?.str("state")?.ifBlank { "idle" } ?: "idle"
                val assignmentId = member?.str("current_assignment_id").orEmpty()
                GroupMemberChip(repository, bot, botName, status, assignmentId.takeIf(String::isNotBlank)?.let { id -> { onOpenTrace(id) } })
            }
        }
        Row(
            Modifier.fillMaxWidth().height(44.dp).clickable { showAnnouncement = !showAnnouncement }.padding(horizontal = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Box(Modifier.width(2.dp).height(28.dp).clip(CircleShape).background(MaterialTheme.colorScheme.primary))
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.Center) {
                Text(stringResource(Res.string.group_announcement), style = MaterialTheme.typography.labelMedium.copy(fontWeight = FontWeight.SemiBold), color = MaterialTheme.colorScheme.primary, maxLines = 1)
                val announcementSummary = announcement?.str("text")?.lineSequence()?.firstOrNull()?.trim().orEmpty()
                Text(announcementSummary.ifBlank { stringResource(Res.string.group_announcement_empty) }, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            LucideIcon(if (showAnnouncement) LucideIconKind.ChevronDown else LucideIconKind.ChevronRight, Modifier.size(18.dp))
        }
        if (showAnnouncement) AnnouncementPanel(announcement, project, onOpenArtifact, onOpenHome, onCopyHome)
        HorizontalDivider()
        Box(Modifier.weight(1f).fillMaxWidth()) {
            chatId?.let { id ->
                ChatScreen(
                    repository = repository,
                    chatId = id,
                    initialMessageId = initialMessageId,
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
            Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                OutlinedTextField(value = changeText, onValueChange = { changeText = it }, modifier = Modifier.weight(1f), singleLine = true, label = { Text(stringResource(Res.string.feature_change_hint)) })
                Button(onClick = {
                    val text = changeText.trim()
                    if (text.isNotEmpty()) {
                        val targetProject = changeProjectId ?: projectId
                        scope.launch { runCatching { repository.call("project.request_changes", buildJsonObject { put("project_id", targetProject); put("text", text) }) }.onFailure { error = it.message } }
                    }
                    changeText = ""
                    changeProjectId = null
                    showChangeInput = false
                }) { Text(stringResource(Res.string.feature_submit_changes)) }
            }
        }
    }
}

@Composable
internal fun GroupAvatar(repository: MobileRepository, members: List<kotlinx.serialization.json.JsonElement>, bots: List<JsonObject>, size: androidx.compose.ui.unit.Dp = 40.dp) {
    val botIds = members.mapNotNull { (it as? JsonObject)?.str("bot_id")?.takeIf(String::isNotBlank) }.distinct().take(4)
    Box(Modifier.size(size)) {
        if (botIds.size == 1) {
            val bot = bots.firstOrNull { it.str("id") == botIds.single() }
            BotAvatar(repository, bot, main = bot?.boolean("is_main") == true, size = size)
        } else {
            botIds.forEachIndexed { index, botId ->
                val quadrant = size / 2f
                val inset = size * 0.03f
                val x = if (index % 2 == 0) 0.dp else quadrant - inset
                val y = if (index / 2 == 0) 0.dp else quadrant - inset
                val avatarSize = quadrant - 2.dp
                Box(
                    Modifier.offset(x, y).size(quadrant).clip(CircleShape).background(MaterialTheme.colorScheme.surface)
                        .padding(1.dp),
                ) {
                    val bot = bots.firstOrNull { it.str("id") == botId }
                    BotAvatar(repository, bot, main = bot?.boolean("is_main") == true, size = avatarSize)
                }
            }
        }
        if (botIds.isEmpty()) {
            Surface(Modifier.fillMaxSize(), shape = CircleShape, color = MaterialTheme.colorScheme.primaryContainer) {
                Box(contentAlignment = Alignment.Center) { LucideIcon(LucideIconKind.MessageCircle, Modifier.size(20.dp), tint = MaterialTheme.colorScheme.onPrimaryContainer) }
            }
        }
    }
}

@Composable
private fun GroupMemberChip(repository: MobileRepository, bot: JsonObject?, name: String, status: String, onAssignmentClick: (() -> Unit)?) {
    val statusLabel = when (status) {
        "working" -> stringResource(Res.string.status_working)
        "queued" -> stringResource(Res.string.status_queued)
        "waiting_user" -> stringResource(Res.string.status_waiting_user)
        "blocked" -> stringResource(Res.string.status_blocked)
        "review" -> stringResource(Res.string.status_review)
        "done" -> stringResource(Res.string.status_done)
        else -> stringResource(Res.string.status_standby)
    }
    val chipColor = when (status) {
        "working", "queued" -> MaterialTheme.colorScheme.primaryContainer
        "blocked", "waiting_user" -> MaterialTheme.colorScheme.errorContainer
        else -> MaterialTheme.colorScheme.surfaceVariant
    }
    val textColor = when (status) {
        "working", "queued" -> MaterialTheme.colorScheme.onPrimaryContainer
        "blocked", "waiting_user" -> MaterialTheme.colorScheme.onErrorContainer
        else -> MaterialTheme.colorScheme.onSurfaceVariant
    }
    Surface(
        color = chipColor,
        contentColor = textColor,
        shape = CircleShape,
        modifier = Modifier.height(26.dp).then(onAssignmentClick?.let { Modifier.clickable(onClick = it) } ?: Modifier),
    ) {
        Row(Modifier.padding(start = 3.dp, end = 9.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            BotAvatar(repository, bot, main = bot?.boolean("is_main") == true, status = status, size = 20.dp)
            Text(name, style = MaterialTheme.typography.labelSmall, maxLines = 1)
            Text(statusLabel, style = MaterialTheme.typography.labelSmall, maxLines = 1)
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
        FontScaleToolbar { Row(Modifier.fillMaxWidth()) { Button(onClick = onBack) { Text(stringResource(Res.string.common_back)) }; Text(stringResource(Res.string.new_create_group), Modifier.padding(start = 12.dp), style = MaterialTheme.typography.titleLarge) } }
        OutlinedTextField(name, { name = it.take(32) }, Modifier.fillMaxWidth(), label = { Text(stringResource(Res.string.feature_group_name)) }, supportingText = { Text(stringResource(Res.string.feature_name_max_length, 32)) })
        OutlinedTextField(goal, { goal = it }, Modifier.fillMaxWidth(), minLines = 3, label = { Text(stringResource(Res.string.feature_announcement_body)) })
        Text(stringResource(Res.string.group_members), style = MaterialTheme.typography.titleSmall)
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
        }, modifier = Modifier.fillMaxWidth()) { Text(stringResource(Res.string.new_create_group)) }
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
            else Text(stringResource(Res.string.group_announcement_empty), color = MaterialTheme.colorScheme.onSurfaceVariant)
            announcement?.str("text_updated_at")?.takeIf { it.isNotBlank() }?.let { Text(stringResource(Res.string.feature_announcement_updated, it), style = MaterialTheme.typography.bodySmall) }
            val home = project?.str("home_path").orEmpty()
            Text(stringResource(Res.string.feature_home_value, home))
            if (home.isNotBlank()) {
                Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    Button(onClick = { onOpenHome(home) }) { Text(stringResource(Res.string.group_open_home)) }
                    Button(onClick = { onCopyHome(home) }) { Text(stringResource(Res.string.common_copy_path)) }
                }
            }
            announcement?.arr("artifacts")?.forEach { item ->
                val artifact = item as? JsonObject ?: return@forEach
                val artifactId = artifact.str("id")
                val pathOrUrl = artifact.str("path_or_url")
                if (artifactId.isNotBlank() && pathOrUrl.isNotBlank()) {
                    Column {
                        Text(stringResource(Res.string.common_provided_by, artifact.str("bot_id").ifBlank { stringResource(Res.string.search_filter_bots) }), style = MaterialTheme.typography.labelSmall)
                        Button(onClick = { onOpenArtifact(artifactId, pathOrUrl, project?.str("id")) }) {
                            Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
                                LucideIcon(LucideIconKind.FileText, Modifier.size(18.dp))
                                Text(artifact.str("title").ifBlank { pathOrUrl })
                            }
                        }
                    }
                }
            }
        }
    }
}
