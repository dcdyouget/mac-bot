package bot.mac.mobile

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.design.*
import bot.mac.mobile.core.ui.FilePreview
import bot.mac.mobile.core.network.ConnectionStatus
import bot.mac.mobile.core.protocol.*
import bot.mac.mobile.core.state.*
import bot.mac.mobile.core.platform.PlatformBackHandler
import bot.mac.mobile.feature.connect.ConnectScreen
import bot.mac.mobile.feature.mainbot.MainBotScreen
import bot.mac.mobile.feature.trace.TraceScreen
import bot.mac.mobile.feature.trace.HistoryAssignmentScreen
import bot.mac.mobile.feature.group.*
import bot.mac.mobile.feature.workbench.WorkbenchScreen
import bot.mac.mobile.feature.bots.*
import bot.mac.mobile.feature.settings.SettingsScreen
import bot.mac.mobile.feature.approval.ApprovalScreen
import bot.mac.mobile.feature.dashboard.DashboardScreen
import bot.mac.mobile.feature.skills.SkillsScreen
import bot.mac.mobile.feature.search.SearchScreen
import bot.mac.mobile.feature.routines.RoutinesScreen
import bot.mac.mobile.feature.computer.ComputerScreen
import bot.mac.mobile.resources.*
import kotlinx.coroutines.launch
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
    var changesProject by remember { mutableStateOf<String?>(null) }
    var changesText by remember { mutableStateOf("") }
    var actionError by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    val clipboard = LocalClipboardManager.current
    fun open(next: String, id: String = "", second: String = "") {
        if (next == page && id == target && second == secondTarget) return
        routeHistory = routeHistory + JsonArray(listOf(JsonPrimitive(page),JsonPrimitive(target),JsonPrimitive(secondTarget))).toString()
        page = next; target = id; secondTarget = second
    }
    val back: () -> Unit = {
        val previous=routeHistory.lastOrNull()
        if(previous==null) { page="home";target="";secondTarget="" }
        else {
            val route=protocolJson.parseToJsonElement(previous).jsonArray
            routeHistory=routeHistory.dropLast(1)
            page=route[0].jsonPrimitive.content;target=route[1].jsonPrimitive.content;secondTarget=route[2].jsonPrimitive.content
        }
    }
    fun openChat(id: String) {
        val chat = repository.state.value.chats.firstOrNull { it.str("id") == id }
        if (chat?.str("kind") == "project") open("group",chat.str("project_id")) else open("chat",id)
    }
    val artifactMissing = stringResource(Res.string.artifact_not_found)
    val openArtifact: (String,String,String?) -> Unit = { id,path,projectId ->
        if(path.startsWith("http://") || path.startsWith("https://") || !projectId.isNullOrBlank()) open("file",projectId.orEmpty(),path)
        else scope.launch { runCatching {
            var found=false
            for(project in repository.call("project.list").objects("projects")) {
                val pid=project.str("id")
                val artifact=repository.call("project.get",jsonParams("project_id" to pid)).obj("announcement").objects("artifacts").firstOrNull{it.str("artifact_id")==id}
                if(artifact!=null) { open("file",pid,artifact.str("path_or_url"));found=true;break }
            }
            if(!found) actionError=artifactMissing
        }.onFailure{actionError=it.message} }
    }
    LaunchedEffect(Unit) { repository.initialize() }
    LaunchedEffect(deepLink) {
        val link = deepLink ?: return@LaunchedEffect
        repository.initialize()
        val hostId=link.substringAfter("host_id=","").substringBefore('&')
        if(hostId.isNotBlank()) {
            if(repository.hosts.value.none { it.id == hostId }) { open("connect");repository.deepLink.value=null;return@LaunchedEffect }
            repository.selectHost(hostId)
        }
        val kind = link.substringAfter("://").substringBefore('/')
        val id = link.substringAfterLast('/').substringBefore('?')
        open(when(kind) { "approval" -> "approvals"; "review" -> "group"; "computer" -> "computer"; else -> "chat" }, id)
        repository.deepLink.value = null
    }
    PlatformBackHandler(page != "home", back)
    MacBotTheme(theme) {
        changesProject?.let { projectId -> AlertDialog(onDismissRequest={changesProject=null},title={Text(stringResource(Res.string.review_changes_title))},text={Column { OutlinedTextField(changesText,{changesText=it},label={Text(stringResource(Res.string.review_changes_hint))});actionError?.let{Text(it,color=MaterialTheme.colorScheme.error)} }},confirmButton={TextButton(enabled=changesText.isNotBlank(),onClick={scope.launch{runCatching{repository.call("project.request_changes",jsonParams("project_id" to projectId,"text" to changesText))}.onSuccess{changesProject=null}.onFailure{actionError=it.message}}}){Text(stringResource(Res.string.review_changes_submit))}},dismissButton={TextButton(onClick={changesProject=null}){Text(stringResource(Res.string.common_cancel))}}) }
        Surface(Modifier.fillMaxSize()) {
            Scaffold(
                bottomBar = {
                    if (page == "home" && hosts.isNotEmpty()) NavigationBar(containerColor = MaterialTheme.colorScheme.surfaceContainer) {
                        listOf(Res.string.tab_messages, Res.string.tab_workbench, Res.string.tab_mine).forEachIndexed { index, label ->
                            NavigationBarItem(selected = tab == index, onClick = { tab = index }, icon = { Text(listOf("▢", "▦", "⚙")[index]) }, label = { Text(stringResource(label)) })
                        }
                    }
                }
            ) { padding ->
                Column(Modifier.padding(padding).imePadding()) {
                    actionError?.let { Text(it,Modifier.fillMaxWidth().padding(8.dp),color=MaterialTheme.colorScheme.error) }
                    if (hosts.isNotEmpty() && !state.connected && page != "connect") {
                        val message = when {
                            connection.error?.message?.contains("401") == true -> Res.string.host_unauthorized
                            connection.error?.message?.contains("403") == true -> Res.string.host_setup
                            connection.error?.message?.contains("protocol", ignoreCase = true) == true -> Res.string.host_version
                            connection.status == ConnectionStatus.CONNECTING -> Res.string.host_connecting
                            else -> Res.string.host_reconnecting
                        }
                        Text(stringResource(message), Modifier.fillMaxWidth().background(DesignTokens.attention.copy(alpha=.12f)).clickable { open("connect") }.padding(8.dp), style = MaterialTheme.typography.bodySmall)
                    }
                    when {
                        page == "connect" || hosts.isEmpty() -> ConnectScreen(repository, back)
                        page == "chat" -> MainBotScreen(repository = repository, chatId = target, onOpenTrace = { assignment, chat -> open("trace", assignment.orEmpty(), chat.orEmpty()) }, onOpenProject = { open("group", it) }, onBack = back, onProjectAction = { projectId, action -> if (action == "confirm_done") scope.launch { runCatching { repository.call("project.confirm_done",jsonParams("project_id" to projectId)) }.onFailure { actionError=it.message } } else { changesProject=projectId;changesText="" } }, onOpenScreen={bot,tabId->open("computer",bot,tabId.orEmpty())},onOpenChat={openChat(it)},onLoopAction={id,action->scope.launch{runCatching{repository.call("loop.resolve",jsonParams("root_message_id" to id,"action" to action))}.onFailure{actionError=it.message}}},onTakeover={open("computer",it)},onOpenArtifact=openArtifact,onOpenHistory={open("history",it)})
                        page == "group" -> GroupScreen(repository = repository, projectId = target, onOpenTrace = { open("trace", it) }, onBack = back,onOpenScreen={bot,tabId->open("computer",bot,tabId.orEmpty())},onOpenChat={openChat(it)},onLoopAction={id,action->scope.launch{runCatching{repository.call("loop.resolve",jsonParams("root_message_id" to id,"action" to action))}.onFailure{actionError=it.message}}},onTakeover={open("computer",it)},onOpenProject={open("group",it)},onOpenArtifact=openArtifact,onOpenHistory={open("history",it)},onOpenHome={open("file",target,"")},onCopyHome={clipboard.setText(AnnotatedString(it))})
                        page == "create_group" -> GroupCreateScreen(repository, onCreated = { open("group", it) }, onBack = back)
                        page == "history" -> HistoryAssignmentScreen(repository,chatId=target.ifBlank{null},onOpenTrace={open("trace",it)},onBack=back)
                        page == "trace" -> TraceScreen(repository, target.ifBlank { null }, secondTarget.ifBlank { null }, { bot, tabId -> open("computer", bot, tabId.orEmpty()) }, onBack=back, onSteer={assignmentId,text ->
                            val assignment=repository.state.value.assignments.firstOrNull{it.str("id")==assignmentId}
                            val hostId=repository.activeHost.value?.id
                            if(assignment!=null && hostId!=null) scope.launch { runCatching { repository.callOnHost(hostId,"chat.send",buildJsonObject {
                                put("chat_id",assignment.str("chat_id"));put("text",text)
                                put("mentions",JsonArray(listOf(buildJsonObject{put("kind","bot");put("bot_id",assignment.str("bot_id"));put("instruction",JsonNull)})))
                            }) }.onFailure{actionError=it.message} }
                        })
                        page == "bots" -> BotsScreen(repository, { open("bot_editor", it) }, { open("bot_editor") }, back)
                        page == "bot_editor" -> BotEditorScreen(repository, target.ifBlank { null }, { open("bots") }, back)
                        page == "settings" -> SettingsScreen(repository, back)
                        page == "approvals" -> ApprovalScreen(repository, back)
                        page == "file" -> FilePreview(repository, target, secondTarget, back)
                        page == "dashboard" -> DashboardScreen(repository, back, { open("connect") })
                        page == "skills" -> SkillsScreen(repository, back)
                        page == "routines" -> RoutinesScreen(repository, { open("trace", it) }, back)
                        page == "computer" -> ComputerScreen(repository, target, secondTarget.ifBlank { null }, back)
                        page == "search" -> SearchScreen(repository, { result ->
                            when (result.str("kind")) {
                                "bot" -> open("bot_editor", result.str("id")); "routine" -> open("routines");
                                "artifact" -> {
                                    scope.launch { runCatching {
                                        val projects=repository.call("project.list").objects("projects")
                                        var found=false
                                        for(project in projects) {
                                            val projectId=project.str("id")
                                            val artifact=repository.call("project.get",jsonParams("project_id" to projectId)).obj("announcement").objects("artifacts").firstOrNull{it.str("artifact_id")==result.str("id")}
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
                            else -> MineScreen { open(it) }
                        }
                    }
                }
            }
        }
    }
}

@Composable private fun MessagesScreen(repository: ClientRepository, onChat: (String)->Unit, onHost: ()->Unit, onSearch: ()->Unit, onGroup: ()->Unit, onBot: ()->Unit) {
    val state by repository.state.collectAsState()
    val active by repository.activeHost.collectAsState()
    var add by remember { mutableStateOf(false) }
    var completed by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    Column(Modifier.fillMaxSize()) {
        Row(Modifier.fillMaxWidth().padding(horizontal=16.dp, vertical=8.dp), verticalAlignment=Alignment.CenterVertically) {
            TextButton(onClick=onHost, modifier=Modifier.weight(1f)) { Text("${active?.name.orEmpty()} ⌄", style=MaterialTheme.typography.titleMedium) }
            TextButton(onClick=onSearch) { Text("⌕", style=MaterialTheme.typography.headlineSmall) }
            Box { TextButton(onClick={add=true}) { Text("＋",style=MaterialTheme.typography.headlineSmall) }; DropdownMenu(expanded=add,onDismissRequest={add=false}) {
                DropdownMenuItem(text={Text(stringResource(Res.string.new_group))},onClick={add=false;onGroup()})
                DropdownMenuItem(text={Text(stringResource(Res.string.new_bot))},onClick={add=false;onBot()})
            } }
        }
        HorizontalDivider()
        LazyColumn(Modifier.weight(1f), contentPadding=PaddingValues(vertical=8.dp)) {
            val main = state.chats.firstOrNull { it.str("kind")=="main" }
            main?.let { item { ChatRow(it, state, onChat, repository) } }
            item { Text(stringResource(Res.string.chat_groups),Modifier.padding(16.dp),style=MaterialTheme.typography.titleMedium) }
            val groups=state.chats.filter { it.str("kind")=="project" }.sortedWith(compareByDescending<JsonObject>{it.boolean("pinned")}.thenByDescending{it.str("updated_at")})
            val visible=groups.filter { chat -> val status=state.projects.firstOrNull { it.str("id")==chat.str("project_id") }?.str("status"); status !in listOf("done","archived") || completed }
            items(visible,key={it.str("id")}) { ChatRow(it,state,onChat,repository) }
            if (groups.isEmpty()) item { Text(stringResource(Res.string.chat_empty),Modifier.padding(horizontal=16.dp,vertical=12.dp),color=MaterialTheme.colorScheme.onSurfaceVariant) }
            item { TextButton(onClick={completed=!completed}) { Text(stringResource(Res.string.chat_completed)+" ▾") } }
            item { Text(stringResource(Res.string.chat_bots),Modifier.padding(16.dp),style=MaterialTheme.typography.titleMedium) }
            items(state.chats.filter { it.str("kind") in listOf("direct","bot_dm") && state.bots.firstOrNull { b->b.str("id")==it.str("bot_id") }?.boolean("hidden")!=true }.sortedByDescending{it.boolean("pinned")},key={it.str("id")}) { ChatRow(it,state,onChat,repository) }
        }
    }
}
@Composable private fun ChatRow(chat:JsonObject,state:MobileState,onChat:(String)->Unit,repository:ClientRepository) {
    val bot=state.bots.firstOrNull { it.str("id")==chat.str("bot_id") }
    val attention=chat.str("attention")
    var menu by remember { mutableStateOf(false) }
    val scope=rememberCoroutineScope()
    Row(Modifier.fillMaxWidth().clickable { onChat(chat.str("id")) }.padding(horizontal=16.dp,vertical=10.dp),verticalAlignment=Alignment.CenterVertically,horizontalArrangement=Arrangement.spacedBy(12.dp)) {
        BotAvatar(repository,bot,chat.str("kind")=="main",bot?.obj("status")?.str("summary") ?: attention)
        Column(Modifier.weight(1f)) {
            Text(chat.str("title"),style=MaterialTheme.typography.titleMedium)
            Text(chat.obj("last_message").str("text"),maxLines=1,color=MaterialTheme.colorScheme.onSurfaceVariant,style=MaterialTheme.typography.bodySmall)
        }
        Column(horizontalAlignment=Alignment.End) {
            if(attention in listOf("waiting_user","review","blocked","working")) Text(when(attention){"waiting_user"->"⚠";"review"->"◎";"blocked"->"!";else->"⟳"},color=if(attention in listOf("waiting_user","review")) DesignTokens.attention else DesignTokens.accent)
            if(chat.long("unread")>0) Badge { Text(chat.long("unread").toString()) }
        }
        Box { TextButton(onClick={menu=true}) { Text("⋮") }; DropdownMenu(menu,{menu=false}) {
            DropdownMenuItem(text={Text(stringResource(Res.string.chat_pin))},onClick={menu=false;scope.launch{runCatching{repository.call("chat.set_pinned",buildJsonObject{put("chat_id",chat.str("id"));put("pinned",!chat.boolean("pinned"))})}}})
            DropdownMenuItem(text={Text(stringResource(Res.string.chat_mute))},onClick={menu=false;scope.launch{runCatching{repository.call("chat.set_muted",buildJsonObject{put("chat_id",chat.str("id"));put("muted",!chat.boolean("muted"))})}}})
        } }
    }
}
@Composable private fun MineScreen(onOpen:(String)->Unit) {
    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.surfaceContainer)) {
        Text(stringResource(Res.string.tab_mine),Modifier.padding(24.dp),style=MaterialTheme.typography.headlineSmall)
        Text(stringResource(Res.string.mine_subtitle),Modifier.padding(horizontal=24.dp),style=MaterialTheme.typography.bodySmall,color=MaterialTheme.colorScheme.onSurfaceVariant)
        LazyColumn(Modifier.weight(1f),contentPadding=PaddingValues(16.dp)) {
            items(listOf("connect" to Res.string.nav_hosts,"bots" to Res.string.nav_bots,"approvals" to Res.string.nav_approval,"dashboard" to Res.string.nav_dashboard,"skills" to Res.string.nav_skills,"routines" to Res.string.nav_routines,"settings" to Res.string.nav_settings)) { (route,label)->
                ListItem(headlineContent={Text(stringResource(label))},trailingContent={Text("›")},modifier=Modifier.clickable{onOpen(route)})
                HorizontalDivider()
            }
        }
        Text(stringResource(Res.string.about_version),Modifier.align(Alignment.CenterHorizontally).padding(16.dp),style=MaterialTheme.typography.labelSmall,color=MaterialTheme.colorScheme.onSurfaceVariant)
    }
}
