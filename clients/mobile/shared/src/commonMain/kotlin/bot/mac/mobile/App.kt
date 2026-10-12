package bot.mac.mobile

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.*
import androidx.compose.material3.*
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.graphics.compositeOver
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.IntOffset
import kotlin.math.roundToInt
import bot.mac.mobile.core.design.*
import bot.mac.mobile.core.ui.FilePreview
import bot.mac.mobile.core.network.ConnectionStatus
import bot.mac.mobile.core.network.MainEvent
import bot.mac.mobile.core.protocol.*
import bot.mac.mobile.core.state.*
import bot.mac.mobile.core.platform.PlatformBackHandler
import bot.mac.mobile.core.platform.DeepLinkKind
import bot.mac.mobile.core.platform.parseMobileDeepLink
import bot.mac.mobile.core.platform.resolveHost
import bot.mac.mobile.core.platform.containsDeepLinkResource
import bot.mac.mobile.core.platform.platformLocaleTag
import bot.mac.mobile.feature.connect.ConnectScreen
import bot.mac.mobile.feature.connect.OnboardingTemplatesScreen
import bot.mac.mobile.feature.mainbot.MainBotScreen
import bot.mac.mobile.feature.mainbot.FollowupScreen
import bot.mac.mobile.feature.trace.TraceScreen
import bot.mac.mobile.feature.trace.HistoryAssignmentScreen
import bot.mac.mobile.feature.trace.buildSteerChatParams
import bot.mac.mobile.feature.group.*
import bot.mac.mobile.feature.workbench.WorkbenchScreen
import bot.mac.mobile.feature.bots.*
import bot.mac.mobile.feature.settings.SettingsScreen
import bot.mac.mobile.feature.memory.MemoryScreen
import bot.mac.mobile.feature.memory.MemoryEntryEditorScreen
import bot.mac.mobile.feature.memory.MemoryProfileEditScreen
import bot.mac.mobile.feature.approval.ApprovalScreen
import bot.mac.mobile.feature.dashboard.DashboardScreen
import bot.mac.mobile.feature.skills.SkillsScreen
import bot.mac.mobile.feature.search.SearchScreen
import bot.mac.mobile.feature.routines.RoutinesScreen
import bot.mac.mobile.feature.computer.ComputerScreen
import bot.mac.mobile.feature.computer.shouldReleaseTakeoverOnHostChange
import bot.mac.mobile.feature.computer.shouldReleaseTakeoverOnNavigation
import bot.mac.mobile.resources.*
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.serialization.json.*
import org.jetbrains.compose.resources.stringResource

@Composable fun App(repository: ClientRepository = AppRuntime.repository) {
    val state by repository.state.collectAsState()
    val hosts by repository.hosts.collectAsState()
    val active by repository.activeHost.collectAsState()
    val connection by repository.connectionStatus.collectAsState()
    val theme by repository.theme.collectAsState()
    val deepLink by repository.deepLink.collectAsState()
    var tab by rememberSaveable { mutableStateOf(0) }
    var page by rememberSaveable { mutableStateOf("home") }
    var target by rememberSaveable { mutableStateOf("") }
    var secondTarget by rememberSaveable { mutableStateOf("") }
    var routeHistory by rememberSaveable { mutableStateOf(listOf<String>()) }
    var computerTakeoverOwnerHostId by rememberSaveable { mutableStateOf<String?>(null) }
    var computerExitInFlight by remember { mutableStateOf(false) }
    var changesProject by remember { mutableStateOf<String?>(null) }
    var changesText by remember { mutableStateOf("") }
    var actionError by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    val takeoverReleaseMutex = remember { Mutex() }
    val clipboard = LocalClipboardManager.current
    val snackbarHostState = remember { SnackbarHostState() }
    val attentionMessage = stringResource(Res.string.feature_attention_banner)
    val attentionAction = stringResource(Res.string.feature_attention_open)
    suspend fun releaseOwnedTakeover(ownerHostId: String, botId: String) {
        takeoverReleaseMutex.withLock {
            if (computerTakeoverOwnerHostId == ownerHostId) {
                runCatching { repository.callOnHost(ownerHostId, "takeover.release", jsonParams("bot_id" to botId)) }
                computerTakeoverOwnerHostId = null
            }
        }
    }
    fun navigate(next: String, id: String = "", second: String = "", pushHistory: Boolean, popHistory: Boolean = false) {
        val routeChanged = next != page || id != target || second != secondTarget
        val releaseTakeover = shouldReleaseTakeoverOnNavigation(
            page, target, next, id, computerTakeoverOwnerHostId, repository.activeHost.value?.id,
        )
        if (!routeChanged && !releaseTakeover) return
        if (computerExitInFlight) return
        val changeRoute = {
            if (pushHistory) routeHistory = routeHistory + JsonArray(listOf(JsonPrimitive(page),JsonPrimitive(target),JsonPrimitive(secondTarget))).toString()
            if (popHistory && routeHistory.isNotEmpty()) routeHistory = routeHistory.dropLast(1)
            page = next; target = id; secondTarget = second
        }
        if (releaseTakeover) {
            val ownerHostId = computerTakeoverOwnerHostId ?: return
            val botId = target
            computerExitInFlight = true
            scope.launch {
                releaseOwnedTakeover(ownerHostId, botId)
                computerExitInFlight = false
                if (routeChanged) changeRoute()
            }
        } else if (routeChanged) {
            changeRoute()
        }
    }
    fun open(next: String, id: String = "", second: String = "") {
        navigate(next, id, second, pushHistory = true)
    }
    val back: () -> Unit = {
        val previous=routeHistory.lastOrNull()
        if(previous==null) navigate("home", "", "", pushHistory = false)
        else {
            val route=protocolJson.parseToJsonElement(previous).jsonArray
            navigate(route[0].jsonPrimitive.content, route[1].jsonPrimitive.content, route[2].jsonPrimitive.content, pushHistory = false, popHistory = true)
        }
    }
    fun openChat(id: String, messageId: String? = null) {
        val chat = repository.state.value.chats.firstOrNull { it.str("id") == id }
        if (chat?.str("kind") == "project") open("group", chat.str("project_id"), messageId.orEmpty())
        else open("chat", id, messageId.orEmpty())
    }
    val artifactMissing = stringResource(Res.string.artifact_not_found)
    val openArtifact: (String,String,String?) -> Unit = { id,path,projectId ->
        if(path.startsWith("http://") || path.startsWith("https://") || !projectId.isNullOrBlank()) open("file",projectId.orEmpty(),path)
        else scope.launch { runCatching {
            var found=false
            for(project in repository.call("project.list").objects("projects")) {
                val pid=project.str("id")
                val artifact=repository.call("project.get",jsonParams("project_id" to pid)).obj("announcement").objects("artifacts").firstOrNull{it.str("id")==id}
                if(artifact!=null) { open("file",pid,artifact.str("path_or_url"));found=true;break }
            }
            if(!found) actionError=artifactMissing
        }.onFailure{actionError=it.message} }
    }
    LaunchedEffect(Unit) { repository.initialize() }
    LaunchedEffect(active?.id, page, target, computerTakeoverOwnerHostId) {
        val ownerHostId = computerTakeoverOwnerHostId ?: return@LaunchedEffect
        if (shouldReleaseTakeoverOnHostChange(page, ownerHostId, active?.id)) {
            releaseOwnedTakeover(ownerHostId, target)
        }
    }
    LaunchedEffect(active?.id, page, target) {
        repository.hostEvents.collect { hostEvent ->
            if (hostEvent.hostId != active?.id) return@collect
            val eventName: String
            val data: JsonObject
            when (val event = hostEvent.event) {
                is MainEvent.Persistent -> { eventName = event.event; data = event.data }
                is MainEvent.Ephemeral -> { eventName = event.event; data = event.data }
                is MainEvent.Hello -> return@collect
            }
            val approval = data.obj("approval")
            val question = data.obj("question")
            val project = data.obj("project")
            val botId = data.str("bot_id").ifBlank { approval.str("bot_id") }
            val projectId = data.str("project_id").ifBlank { project.str("id") }
            val chatId = data.str("chat_id").ifBlank { approval.str("chat_id").ifBlank { question.str("chat_id") } }
            val needsYou = eventName in setOf("approval.requested", "question.asked", "takeover.requested") ||
                (eventName == "project.updated" && project.str("status") == "review")
            if (!needsYou) return@collect
            val currentChat = when (page) {
                "chat" -> target
                "group" -> hostEvent.state.chats.firstOrNull { it.str("kind") == "project" && it.str("project_id") == target }?.str("id").orEmpty()
                else -> ""
            }
            if (chatId.isNotBlank() && chatId == currentChat) return@collect
            val destination = when (eventName) {
                "approval.requested", "question.asked" -> "approvals"
                "takeover.requested" -> "computer"
                else -> "group"
            }
            val destinationId = when (destination) {
                "computer" -> botId
                "group" -> projectId
                else -> approval.str("id").ifBlank { question.str("id") }
            }
            snackbarHostState.currentSnackbarData?.dismiss()
            val result = snackbarHostState.showSnackbar(attentionMessage, attentionAction, duration = SnackbarDuration.Short)
            if (result == SnackbarResult.ActionPerformed) open(destination, destinationId)
        }
    }
    LaunchedEffect(deepLink) {
        val value = deepLink ?: return@LaunchedEffect
        val link = parseMobileDeepLink(value)
        if (link == null) {
            repository.deepLink.value = null
            return@LaunchedEffect
        }
        repository.initialize()
        val hostId = link.resolveHost(
            repository.hosts.value.map { it.id }.toSet(),
            repository.activeHost.value?.id,
        )
        if (hostId == null) {
            repository.deepLink.value = null
            return@LaunchedEffect
        }
        repository.selectHost(hostId)
        if (!repository.state.value.containsDeepLinkResource(link)) {
            repository.deepLink.value = null
            return@LaunchedEffect
        }
        when (link.kind) {
            DeepLinkKind.APPROVAL -> open("approvals", link.resourceId)
            DeepLinkKind.REVIEW -> open("group", link.resourceId)
            DeepLinkKind.CHAT -> openChat(link.resourceId, link.messageId)
            DeepLinkKind.COMPUTER -> open("computer", link.resourceId)
        }
        repository.deepLink.value = null
    }
    PlatformBackHandler(page != "home", back)
    MacBotTheme(theme) {
        changesProject?.let { projectId -> AlertDialog(onDismissRequest={changesProject=null},title={Text(stringResource(Res.string.review_changes_title))},text={Column { OutlinedTextField(changesText,{changesText=it},label={Text(stringResource(Res.string.review_changes_hint))});actionError?.let{Text(it,color=MaterialTheme.colorScheme.error)} }},confirmButton={TextButton(enabled=changesText.isNotBlank(),onClick={scope.launch{runCatching{repository.call("project.request_changes",jsonParams("project_id" to projectId,"text" to changesText))}.onSuccess{changesProject=null}.onFailure{actionError=it.message}}}){Text(stringResource(Res.string.review_changes_submit))}},dismissButton={TextButton(onClick={changesProject=null}){Text(stringResource(Res.string.common_cancel))}}) }
        Surface(Modifier.fillMaxSize()) {
            Scaffold(
                containerColor = LocalMobilePalette.current.window,
                snackbarHost = { SnackbarHost(snackbarHostState) },
                bottomBar = {
                    if (page == "home" && hosts.isNotEmpty()) FontScaleLimit(STANDARD_FONT_SCALE_LIMIT) {
                        NavigationBar(
                            modifier = Modifier.height(80.dp),
                            containerColor = LocalMobilePalette.current.sidebar,
                            contentColor = LocalMobilePalette.current.textSecondary,
                        ) {
                            listOf(Res.string.nav_tab_messages, Res.string.nav_tab_workbench, Res.string.nav_tab_me).forEachIndexed { index, label ->
                                NavigationBarItem(
                                    selected = tab == index, onClick = { tab = index },
                                    icon = { Text(listOf("◉", "⟳", "○")[index], style = MaterialTheme.typography.titleMedium) },
                                    label = { Text(stringResource(label)) },
                                    colors = NavigationBarItemDefaults.colors(
                                        selectedIconColor = LocalMobilePalette.current.accent,
                                        selectedTextColor = LocalMobilePalette.current.textPrimary,
                                        indicatorColor = LocalMobilePalette.current.accentSoft,
                                    ),
                                )
                            }
                        }
                    }
                }
            ) { padding ->
                Column(Modifier.padding(padding).imePadding()) {
                    actionError?.let { Text(it,Modifier.fillMaxWidth().semantics { liveRegion = LiveRegionMode.Polite }.padding(8.dp),color=MaterialTheme.colorScheme.error) }
                    if (hosts.isNotEmpty() && !state.connected && page != "connect") {
                        val message = when {
                            connection.error?.message?.contains("401") == true -> Res.string.host_unauthorized
                            connection.error?.message?.contains("403") == true -> Res.string.host_setup
                            connection.error?.message?.contains("protocol", ignoreCase = true) == true -> Res.string.host_version
                            connection.status == ConnectionStatus.CONNECTING -> Res.string.host_connecting
                            else -> Res.string.host_reconnecting_generic
                        }
                        Text(stringResource(message), Modifier.fillMaxWidth().semantics { liveRegion = LiveRegionMode.Polite }.background(LocalMobilePalette.current.warningSoft).clickable { open("connect") }.padding(horizontal = 16.dp, vertical = 8.dp), style = MaterialTheme.typography.bodySmall, color = LocalMobilePalette.current.warning)
                    }
                    when {
                        page == "connect" || hosts.isEmpty() -> ConnectScreen(
                            repository = repository,
                            onBack = back,
                            onFirstHostSaved = { hostId ->
                                routeHistory = emptyList()
                                page = "onboarding_templates"
                                target = hostId
                                secondTarget = ""
                            },
                        )
                        page == "onboarding_templates" -> OnboardingTemplatesScreen(
                            repository = repository,
                            onboardingHostId = target,
                            onFinished = { hostId ->
                                if (repository.activeHost.value?.id == hostId) {
                                    val mainChatId = repository.state.value.chats.firstOrNull { it.str("kind") == "main" }?.str("id").orEmpty()
                                    routeHistory = emptyList()
                                    page = if (mainChatId.isBlank()) "home" else "chat"
                                    target = mainChatId
                                    secondTarget = ""
                                    tab = 0
                                }
                            },
                            onBack = {
                                routeHistory = emptyList()
                                page = "connect"
                                target = ""
                                secondTarget = ""
                            },
                        )
                        page == "chat" -> MainBotScreen(repository = repository, chatId = target, initialMessageId = secondTarget, onOpenTrace = { assignment, chat -> open("trace", assignment.orEmpty(), chat.orEmpty()) }, onOpenProject = { open("group", it) }, onBack = back, onProjectAction = { projectId, action -> if (action == "confirm_done") scope.launch { runCatching { repository.call("project.confirm_done",jsonParams("project_id" to projectId)) }.onFailure { actionError=it.message } } else { changesProject=projectId;changesText="" } }, onOpenScreen={bot,tabId->open("computer",bot,tabId.orEmpty())},onOpenChat={openChat(it)},onLoopAction={id,action->scope.launch{runCatching{repository.call("loop.resolve",jsonParams("root_message_id" to id,"action" to action))}.onFailure{actionError=it.message}}},onTakeover={open("computer",it)},onOpenArtifact=openArtifact,onOpenHistory={open("history",it)},onOpenFollowup={open("followup",it)},onCreateSkill={ open("skills", "", it) })
                        page == "group" -> GroupScreen(repository = repository, projectId = target, initialMessageId = secondTarget.takeIf { it != "edit_announcement" }, onOpenTrace = { open("trace", it) }, onBack = back,onOpenScreen={bot,tabId->open("computer",bot,tabId.orEmpty())},onOpenChat={openChat(it)},onLoopAction={id,action->scope.launch{runCatching{repository.call("loop.resolve",jsonParams("root_message_id" to id,"action" to action))}.onFailure{actionError=it.message}}},onTakeover={open("computer",it)},onOpenProject={open("group",it)},onOpenArtifact=openArtifact,onOpenHistory={open("history",it)},onOpenHome={open("file",target,"")},onCopyHome={clipboard.setText(AnnotatedString(it))},onOpenGroupInfo={open("groupinfo",it)},onCreateSkill={ open("skills", "", it) },editAnnouncementOnOpen=secondTarget=="edit_announcement")
                        page == "groupinfo" -> GroupInfoScreen(repository, target, back, onEditAnnouncement = { open("group", it, "edit_announcement") }, onOpenHome = { open("file", target, it) }, onCopyHome = { clipboard.setText(AnnotatedString(it)) }, onOpenArtifact = openArtifact)
                        page == "followup" -> FollowupScreen(repository, target, onBack = back, onOpenProject = { open("group", it) }, onOpenChat = { openChat(it) }, onOpenTrace = { assignment, chat -> open("trace", assignment, chat.orEmpty()) }, onOpenApproval = { open("approvals") }, onOpenQuestion = { id ->
                            val question = repository.state.value.questions.firstOrNull { it.str("id") == id }
                            question?.str("chat_id")?.takeIf(String::isNotBlank)?.let { openChat(it) } ?: open("approvals")
                        })
                        page == "create_group" -> GroupCreateScreen(repository, onCreated = { open("group", it) }, onBack = back)
                        page == "history" -> HistoryAssignmentScreen(repository,chatId=target.ifBlank{null},botId=secondTarget.ifBlank{null},onOpenTrace={open("trace",it)},onBack=back)
                        page == "trace" -> TraceScreen(repository, target.ifBlank { null }, secondTarget.ifBlank { null }, { bot, tabId -> open("computer", bot, tabId.orEmpty()) }, onBack=back, onOpenAssignmentTrace = { open("trace", it, secondTarget) }, onSteer={assignmentId,text ->
                            val assignment=repository.state.value.assignments.firstOrNull{it.str("id")==assignmentId}
                            val hostId=repository.activeHost.value?.id
                            if(assignment!=null && hostId!=null) scope.launch { runCatching { repository.callOnHost(hostId,"chat.send",buildSteerChatParams(assignment, text, platformLocaleTag())) }.onFailure{actionError=it.message} }
                        })
                        page == "bots" -> BotsScreen(repository, { open("bot_editor", it) }, { open("bot_editor") }, back, onOpenDetail = { open("bot_detail", it) })
                        page == "bot_detail" -> BotDetailScreen(
                            repository = repository,
                            botId = target,
                            onOpenChat = { openChat(it) },
                            onOpenScreen = { open("computer", it) },
                            onOpenRoutines = { open("routines") },
                            onOpenHistory = { open("history", "", it) },
                            onOpenGroup = { open("group", it) },
                            onOpenAssignment = { open("trace", it) },
                            onEdit = { open("bot_editor", it) },
                            onBack = back,
                        )
                        page == "bot_editor" -> BotEditorScreen(repository, target.ifBlank { null }, { open("bots") }, back)
                        page == "settings" -> SettingsScreen(repository, back)
                        page == "memory" -> MemoryScreen(repository, back, { open("memory_edit", it) }, { open("profile_edit", it) }, { chatId, messageId -> openChat(chatId, messageId) })
                        page == "memory_edit" -> MemoryEntryEditorScreen(repository, target, back) { chatId, messageId -> openChat(chatId, messageId) }
                        page == "profile_edit" -> MemoryProfileEditScreen(repository, target, back)
                        page == "approvals" -> ApprovalScreen(repository, back, target.ifBlank { null })
                        page == "file" -> FilePreview(repository, target, secondTarget, back)
                        page == "dashboard" -> DashboardScreen(repository, back, { open("connect") })
                        page == "skills" -> SkillsScreen(repository, back, secondTarget.takeIf(String::isNotBlank))
                        page == "routines" -> RoutinesScreen(repository, { open("trace", it) }, back)
                        page == "computer" -> ComputerScreen(repository, target, secondTarget.ifBlank { null }, back) { computerTakeoverOwnerHostId = it }
                        page == "search" -> SearchScreen(repository, { result ->
                            when (result.str("kind")) {
                                "bot" -> open("bot_editor", result.str("id")); "routine" -> open("routines");
                                "artifact" -> {
                                    scope.launch { runCatching {
                                        val projects=repository.call("project.list").objects("projects")
                                        var found=false
                                        for(project in projects) {
                                            val projectId=project.str("id")
                                            val artifact=repository.call("project.get",jsonParams("project_id" to projectId)).obj("announcement").objects("artifacts").firstOrNull{it.str("id")==result.str("id")}
                                            if(artifact!=null) { open("file",artifact.str("root_id").ifBlank{projectId},artifact.str("path_or_url"));found=true;break }
                                        }
                                        if(!found) result.str("chat_id").takeIf{it.isNotBlank()}?.let{openChat(it)}
                                    }.onFailure{actionError=it.message} }
                                }
                                else -> result.str("chat_id").ifBlank { if(result.str("kind") == "chat") result.str("id") else "" }.takeIf { it.isNotBlank() }?.let { openChat(it) }
                            }
                        }, back)
                        else -> when(tab) {
                            0 -> MessagesScreen(repository, { openChat(it) }, { open("connect") }, { open("search") }, { open("create_group") }, { open("bot_editor") })
                            1 -> WorkbenchScreen(repository, { open("trace", it) }, { open("group", it) },onOpenApproval={open("approvals",it)},onOpenQuestion={open("approvals",it)},onOpenComputer={bot,tabId->open("computer",bot,tabId.orEmpty())})
                            else -> MineScreen(repository) { open(it) }
                        }
                    }
                }
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable private fun MessagesScreen(repository: ClientRepository, onChat: (String)->Unit, onHost: ()->Unit, onSearch: ()->Unit, onGroup: ()->Unit, onBot: ()->Unit) {
    val state by repository.state.collectAsState()
    val active by repository.activeHost.collectAsState()
    var newSheet by remember { mutableStateOf(false) }
    var hostSheet by remember { mutableStateOf(false) }
    var completed by remember { mutableStateOf(false) }
    val hostStatuses by repository.hostStatuses.collectAsState()
    val scope = rememberCoroutineScope()
    Box(Modifier.fillMaxSize()) {
    Column(Modifier.fillMaxSize()) {
        FontScaleToolbar {
        Row(Modifier.fillMaxWidth().heightIn(min = 64.dp).padding(horizontal=16.dp), verticalAlignment=Alignment.CenterVertically) {
            TextButton(onClick={hostSheet=true}, modifier=Modifier.weight(1f)) { Text("${active?.name.orEmpty()} ⌄", style=MaterialTheme.typography.titleMedium) }
            TextButton(onClick=onSearch) { Text("⌕", style=MaterialTheme.typography.headlineSmall) }
        }
        }
        HorizontalDivider()
        LazyColumn(Modifier.weight(1f), contentPadding=PaddingValues(vertical=8.dp)) {
            val main = state.chats.firstOrNull { it.str("kind")=="main" }
            main?.let { item { ChatRow(it, state, onChat, repository) } }
            val groups=state.chats.filter { it.str("kind")=="project" }.sortedWith(
                compareBy<JsonObject> { chatAttentionRank(it, state) }
                    .thenByDescending { it.boolean("pinned") }
                    .thenByDescending { it.str("updated_at") },
            )
            item { Text(stringResource(Res.string.sidebar_section_groups, groups.size),Modifier.padding(16.dp),style=MaterialTheme.typography.titleMedium) }
            val visible=groups.filter { chat -> val status=state.projects.firstOrNull { it.str("id")==chat.str("project_id") }?.str("status"); status !in listOf("done","archived") || completed }
            items(visible,key={it.str("id")}) { ChatRow(it,state,onChat,repository) }
            if (groups.isEmpty()) item { Text(stringResource(Res.string.sidebar_empty_groups),Modifier.padding(horizontal=16.dp,vertical=12.dp),color=MaterialTheme.colorScheme.onSurfaceVariant) }
            item { TextButton(onClick={completed=!completed}) { Text(stringResource(Res.string.sidebar_completed)+" ▾") } }
            item { Text(stringResource(Res.string.sidebar_section_bots, state.bots.count { !it.boolean("hidden") }),Modifier.padding(16.dp),style=MaterialTheme.typography.titleMedium) }
            items(state.chats.filter { it.str("kind") in listOf("direct","bot_dm") && state.bots.firstOrNull { b->b.str("id")==it.str("bot_id") }?.boolean("hidden")!=true }.sortedByDescending{it.boolean("pinned")},key={it.str("id")}) { ChatRow(it,state,onChat,repository) }
        }
    }
    androidx.compose.material3.FloatingActionButton(
        onClick = { newSheet = true },
        modifier = Modifier.align(Alignment.BottomEnd).padding(16.dp).size(MobileLayout.fab),
        shape = androidx.compose.foundation.shape.RoundedCornerShape(16.dp),
        containerColor = LocalMobilePalette.current.accent,
        contentColor = MaterialTheme.colorScheme.onPrimary,
    ) { Text("＋", style = MaterialTheme.typography.headlineSmall) }
    }
    if (newSheet) androidx.compose.material3.ModalBottomSheet(onDismissRequest = { newSheet = false }) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(stringResource(Res.string.common_new), style = MaterialTheme.typography.titleMedium)
            TextButton(onClick = { newSheet = false; onGroup() }, modifier = Modifier.fillMaxWidth()) { Text(stringResource(Res.string.new_group_action), Modifier.fillMaxWidth()) }
            TextButton(onClick = { newSheet = false; onBot() }, modifier = Modifier.fillMaxWidth()) { Text(stringResource(Res.string.new_bot_action), Modifier.fillMaxWidth()) }
        }
    }
    if (hostSheet) androidx.compose.material3.ModalBottomSheet(onDismissRequest = { hostSheet = false }) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(stringResource(Res.string.host_switch), style = MaterialTheme.typography.titleMedium)
            repository.hosts.value.forEach { host ->
                TextButton(onClick = {
                    scope.launch { repository.selectHost(host.id); hostSheet = false }
                }, modifier = Modifier.fillMaxWidth()) {
                    Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.Start) {
                        Text("${if (active?.id == host.id) "✓ " else ""}${host.name}", style = MaterialTheme.typography.titleSmall)
                        Text(host.addresses.firstOrNull().orEmpty(), style = MaterialTheme.typography.bodySmall)
                        Text(
                            when (hostStatuses[host.id]?.status) {
                                ConnectionStatus.CONNECTED -> stringResource(Res.string.host_online)
                                ConnectionStatus.CONNECTING, ConnectionStatus.RECONNECTING -> stringResource(Res.string.host_reconnecting_generic)
                                else -> stringResource(Res.string.host_offline)
                            },
                            style = MaterialTheme.typography.labelSmall,
                        )
                    }
                }
            }
            Button(onClick = { hostSheet = false; onHost() }, modifier = Modifier.fillMaxWidth()) { Text(stringResource(Res.string.host_add)) }
        }
    }
}
@Composable private fun ChatRow(chat:JsonObject,state:MobileState,onChat:(String)->Unit,repository:ClientRepository) {
    val bot=state.bots.firstOrNull { it.str("id")==chat.str("bot_id") }
    val attention=chat.str("attention")
    val pendingApproval = state.approvals.firstOrNull { it.str("chat_id") == chat.str("id") && it.str("state") == "pending" }
    val pendingQuestion = state.questions.firstOrNull { it.str("chat_id") == chat.str("id") && it.str("state") == "pending" }
    val attentionLabel = when {
        pendingApproval != null -> stringResource(Res.string.chat_attention_approval, pendingApproval.str("summary").ifBlank { pendingApproval.str("tool") })
        pendingQuestion != null -> stringResource(Res.string.chat_attention_question, pendingQuestion.str("text"))
        attention == "review" -> stringResource(Res.string.chat_attention_review)
        attention == "blocked" -> stringResource(Res.string.chat_attention_blocked)
        attention == "working" -> stringResource(Res.string.chat_attention_working)
        chat.long("unread") > 0 -> stringResource(Res.string.chat_attention_unread, chat.long("unread").toInt())
        attention.isBlank() || attention == "idle" -> stringResource(Res.string.chat_attention_idle)
        else -> stringResource(Res.string.chat_attention_other)
    }
    val rowDescription = stringResource(Res.string.chat_row_accessibility, chat.str("title"), attentionLabel)
    var rowError by remember { mutableStateOf<String?>(null) }
    var swipeOffset by remember(chat.str("id")) { mutableStateOf(0f) }
    val density = LocalDensity.current
    val revealWidth = with(density) { 216.dp.toPx() }
    val swipeThreshold = with(density) { 48.dp.toPx() }
    var suppressNextClick by remember { mutableStateOf(false) }
    val scope=rememberCoroutineScope()
    Box(Modifier.fillMaxWidth().heightIn(min = 72.dp)) {
        Row(Modifier.align(Alignment.CenterEnd).width(216.dp).fillMaxHeight(), horizontalArrangement=Arrangement.spacedBy(0.dp)) {
            val projectId = chat.str("project_id")
            listOf(
                Triple(if (chat.boolean("pinned")) "★" else "☆", if (chat.boolean("pinned")) Res.string.chat_unpin else Res.string.sidebar_menu_pin, "pin"),
                Triple(if (projectId.isNotBlank()) "✓" else "✓", Res.string.sidebar_menu_mark_done, "done"),
                Triple(if (chat.boolean("muted")) "◉" else "◌", if (chat.boolean("muted")) Res.string.chat_unmute else Res.string.sidebar_menu_mute, "mute"),
            ).forEach { (icon, label, action) ->
                Column(
                    Modifier.width(72.dp).fillMaxHeight().background(when (action) { "pin" -> LocalMobilePalette.current.accent; "done" -> LocalMobilePalette.current.success; else -> LocalMobilePalette.current.textSecondary })
                        .clickable {
                            if (action == "done" && projectId.isBlank()) return@clickable
                            scope.launch {
                                runCatching {
                                    when (action) {
                                        "pin" -> repository.call("chat.set_pinned", buildJsonObject { put("chat_id", chat.str("id")); put("pinned", !chat.boolean("pinned")) })
                                        "mute" -> repository.call("chat.set_muted", buildJsonObject { put("chat_id", chat.str("id")); put("muted", !chat.boolean("muted")) })
                                        else -> repository.call("project.confirm_done", buildJsonObject { put("project_id", projectId) })
                                    }
                                }.onFailure { rowError = it.message ?: "" }
                                swipeOffset = 0f
                            }
                        }, verticalArrangement=Arrangement.Center, horizontalAlignment=Alignment.CenterHorizontally,
                ) {
                    Text(icon, color=MaterialTheme.colorScheme.surface, style=MaterialTheme.typography.titleMedium)
                    Text(stringResource(label), color=MaterialTheme.colorScheme.surface, style=MaterialTheme.typography.labelSmall, maxLines=1)
                }
            }
        }
        Column(
            Modifier.fillMaxWidth().offset { IntOffset(swipeOffset.roundToInt(), 0) }
                .background(
                    if (chat.str("kind") == "main") LocalMobilePalette.current.accentSoft.compositeOver(LocalMobilePalette.current.window)
                    else LocalMobilePalette.current.window,
                ),
        ) {
            Row(
            Modifier
            .fillMaxWidth()
            .heightIn(min = 72.dp)
            .pointerInput(chat.str("id"), swipeThreshold, revealWidth) {
                var swipeDistance = 0f
                detectHorizontalDragGestures(
                    onHorizontalDrag = { change, amount ->
                        change.consume()
                        swipeDistance = (swipeDistance + amount).coerceIn(-revealWidth, revealWidth)
                        swipeOffset = (swipeOffset + amount).coerceIn(-revealWidth, 0f)
                    },
                    onDragEnd = {
                        if (swipeDistance <= -swipeThreshold) { swipeOffset = -revealWidth; suppressNextClick = true }
                        else if (swipeDistance >= swipeThreshold) swipeOffset = 0f
                        swipeDistance = 0f
                    },
                    onDragCancel = { swipeDistance = 0f },
                )
            }
            .clickable {
                if (suppressNextClick) suppressNextClick = false else onChat(chat.str("id"))
            }
            .padding(horizontal=16.dp,vertical=10.dp),
        verticalAlignment=Alignment.CenterVertically,
        horizontalArrangement=Arrangement.spacedBy(12.dp),
    ) {
        BotAvatar(repository,bot,chat.str("kind")=="main",bot?.obj("status")?.str("summary") ?: attention, size = 48.dp)
        Column(Modifier.weight(1f)) {
            Text(chat.str("title"),style=MaterialTheme.typography.titleMedium,maxLines=1,overflow=TextOverflow.Ellipsis,modifier=Modifier.semantics { contentDescription = rowDescription })
            when {
                pendingApproval != null -> Text(
                    "⚠ ${stringResource(Res.string.status_need_approval)} · ${pendingApproval.str("summary").ifBlank { pendingApproval.str("tool") }}",
                    maxLines=1, color=LocalMobilePalette.current.warning, style=MaterialTheme.typography.bodySmall,
                )
                pendingQuestion != null -> Text(
                    "⚠ ${stringResource(Res.string.status_waiting_user)} · ${pendingQuestion.str("text")}",
                    maxLines=1, color=LocalMobilePalette.current.warning, style=MaterialTheme.typography.bodySmall,
                )
                attention == "review" -> Text(
                    "◎ ${stringResource(Res.string.status_review_you)}",
                    maxLines=1, color=LocalMobilePalette.current.review, style=MaterialTheme.typography.bodySmall,
                )
                else -> Text(chat.obj("last_message").str("text"),maxLines=1,color=MaterialTheme.colorScheme.onSurfaceVariant,style=MaterialTheme.typography.bodySmall)
            }
        }
        Column(horizontalAlignment=Alignment.End) {
            when {
                pendingApproval != null || pendingQuestion != null || attention == "waiting_user" -> Text(
                    "⚠", color=LocalMobilePalette.current.warning,
                    modifier=Modifier.semantics { contentDescription = attentionLabel },
                )
                attention == "review" -> Text("◎", color=LocalMobilePalette.current.review, modifier=Modifier.semantics { contentDescription = attentionLabel })
                attention == "blocked" -> Text("!", color=LocalMobilePalette.current.danger, modifier=Modifier.semantics { contentDescription = attentionLabel })
                attention == "working" -> Text("⟳", color=LocalMobilePalette.current.accent, modifier=Modifier.semantics { contentDescription = attentionLabel })
                chat.long("unread") > 0 -> Badge(containerColor = if (chat.boolean("muted")) LocalMobilePalette.current.textTertiary else MaterialTheme.colorScheme.error) { Text(chat.long("unread").toString(), modifier=Modifier.semantics { contentDescription = attentionLabel }) }
            }
        }
            }
            rowError?.let { Text(it, Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.errorContainer).padding(horizontal=16.dp, vertical=4.dp), color=MaterialTheme.colorScheme.onErrorContainer, style=MaterialTheme.typography.labelSmall) }
        }
    }
}

private fun chatAttentionRank(chat: JsonObject, state: MobileState): Int = when {
    state.approvals.any { it.str("chat_id") == chat.str("id") && it.str("state") == "pending" } -> 0
    state.questions.any { it.str("chat_id") == chat.str("id") && it.str("state") == "pending" } -> 0
    chat.str("attention") == "waiting_user" -> 0
    chat.str("attention") == "review" -> 1
    chat.str("attention") == "blocked" -> 2
    chat.str("attention") == "working" -> 3
    chat.long("unread") > 0 -> 4
    else -> 5
}
@Composable private fun MineScreen(repository: ClientRepository, onOpen:(String)->Unit) {
    val state by repository.state.collectAsState()
    val activeHost by repository.activeHost.collectAsState()
    LaunchedEffect(activeHost?.id) { if (activeHost != null) runCatching { repository.call("memory.user.list") } }
    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.surfaceContainer)) {
        FontScaleToolbar { Text(stringResource(Res.string.nav_tab_me),Modifier.padding(24.dp),style=MaterialTheme.typography.headlineSmall) }
        Text(stringResource(Res.string.mine_subtitle),Modifier.padding(horizontal=24.dp),style=MaterialTheme.typography.bodySmall,color=MaterialTheme.colorScheme.onSurfaceVariant)
        LazyColumn(Modifier.weight(1f),contentPadding=PaddingValues(16.dp)) {
            items(listOf("routines" to Res.string.nav_routines,"memory" to Res.string.memory_title,"connect" to Res.string.nav_hosts,"bots" to Res.string.nav_bots_manage,"approvals" to Res.string.nav_approval,"dashboard" to Res.string.nav_dashboard,"skills" to Res.string.nav_skills,"settings" to Res.string.nav_settings)) { (route,label)->
                ListItem(
                    headlineContent={Text(stringResource(label))},
                    trailingContent={if (route == "memory" && state.memoryQuota.isNotEmpty()) Text("${state.memoryQuota.long("used_chars")} / ${state.memoryQuota.long("limit_chars")}") else Text("›")},
                    modifier=Modifier.clickable{onOpen(route)},
                )
                HorizontalDivider()
            }
        }
        Text(stringResource(Res.string.about_version),Modifier.align(Alignment.CenterHorizontally).padding(16.dp),style=MaterialTheme.typography.labelSmall,color=MaterialTheme.colorScheme.onSurfaceVariant)
    }
}
