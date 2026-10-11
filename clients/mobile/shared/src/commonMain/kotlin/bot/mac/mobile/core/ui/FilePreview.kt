package bot.mac.mobile.core.ui

import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.platform.*
import bot.mac.mobile.core.protocol.*
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.resources.*
import kotlinx.coroutines.launch
import kotlinx.serialization.json.*
import org.jetbrains.compose.resources.stringResource
import androidx.compose.runtime.collectAsState
import kotlin.io.encoding.Base64

private enum class PreviewTab { Preview, Source, History }

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun FilePreview(repository: MobileRepository, rootId: String, initialPath: String, onBack: () -> Unit) {
    var path by remember(initialPath) { mutableStateOf(initialPath) }
    var content by remember { mutableStateOf<ByteArray?>(null) }
    var entries by remember { mutableStateOf<List<JsonObject>>(emptyList()) }
    var modifiedAt by remember { mutableStateOf("") }
    var error by remember { mutableStateOf<String?>(null) }
    var loading by remember { mutableStateOf(true) }
    var selectedTab by remember { mutableStateOf(PreviewTab.Preview) }
    var historyVersions by remember { mutableStateOf<List<JsonObject>>(emptyList()) }
    var historyLoading by remember { mutableStateOf(false) }
    var historyError by remember { mutableStateOf<String?>(null) }
    var historyHasMore by remember { mutableStateOf(false) }
    var selectedVersion by remember { mutableStateOf<Long?>(null) }
    var selectedVersionContent by remember { mutableStateOf<ByteArray?>(null) }
    val scope = rememberCoroutineScope()
    val clipboard = LocalClipboardManager.current
    val snackbar = remember { SnackbarHostState() }
    val mobileState by repository.state.collectAsState()
    val copiedMessage = stringResource(Res.string.file_preview_copied)
    val failedMessage = stringResource(Res.string.file_preview_action_failed)
    val exportLabel = stringResource(Res.string.file_save)

    LaunchedEffect(rootId, path) {
        loading = true
        content = null
        error = null
        entries = emptyList()
        modifiedAt = ""
        if (path.startsWith("http://") || path.startsWith("https://")) {
            runCatching { openExternalUrl(path) }
                .onFailure { error = it.message }
            loading = false
        } else {
            val params = mapOf("root" to "project", "root_id" to rootId, "path" to path)
            val fetched = runCatching { repository.fetchBytes("/api/v1/files", params) }
            fetched.onSuccess { content = it }
            fetched.onFailure { fileError ->
                runCatching {
                    protocolJson.parseToJsonElement(
                        repository.fetchText("/api/v1/files/list", params),
                    ).jsonObject.objects("entries")
                }.onSuccess { values ->
                    entries = values
                    error = if (values.isEmpty()) fileError.message else null
                }.onFailure { listError -> error = listError.message ?: fileError.message }
            }
            val parentPath = path.substringBeforeLast('/', "")
            if (parentPath != path) {
                runCatching {
                    protocolJson.parseToJsonElement(
                        repository.fetchText(
                            "/api/v1/files/list",
                            mapOf("root" to "project", "root_id" to rootId, "path" to parentPath),
                        ),
                    ).jsonObject.objects("entries")
                }.onSuccess { values ->
                    modifiedAt = values.firstOrNull { it.str("path") == path }?.str("modified_at").orEmpty()
                }
            }
            loading = false
        }
    }

    val bytes = content
    val image = remember(bytes) { bytes?.let { platformScreenImageDecoder().decodeJpeg(it) } }
    val isText = remember(path, bytes) {
        val ext = path.substringAfterLast('.', "").lowercase()
        ext in setOf("md", "markdown", "txt", "json", "csv", "xml", "html", "css", "js", "kt", "rs", "py", "yaml", "yml", "toml", "log") ||
            bytes?.decodeToString()?.take(256)?.contains('\u0000') == false
    }
    val artifact = remember(mobileState.announcements, rootId, path) {
        mobileState.announcements[rootId]?.let { announcement ->
            Announcement(announcement).artifacts.firstOrNull { it.pathOrUrl == path || it.pathOrUrl.trimEnd('/') == path.trimEnd('/') }
        }
    }
    val artifactAuthor = remember(artifact, mobileState.bots) {
        artifact?.botId?.let { botId ->
            mobileState.bots.firstOrNull { it.str("id") == botId }?.let { bot -> bot.str("label").ifBlank { bot.str("name") } }
        }.orEmpty()
    }
    LaunchedEffect(artifact?.id, selectedTab) {
        val artifactId = artifact?.id.orEmpty()
        if (selectedTab != PreviewTab.History || artifactId.isBlank()) return@LaunchedEffect
        historyLoading = true
        historyError = null
        historyVersions = emptyList()
        selectedVersion = null
        selectedVersionContent = null
        runCatching {
            repository.call("artifact.versions.list", buildJsonObject {
                put("artifact_id", artifactId)
                put("limit", 20)
            })
        }.onSuccess { result ->
            historyVersions = result.objects("versions")
            historyHasMore = result.boolean("has_more")
        }.onFailure { historyError = it.message ?: "" }
        historyLoading = false
    }
    val title = artifact?.title?.takeIf { it.isNotBlank() } ?: path.trimEnd('/').substringAfterLast('/').ifBlank { path }
    val timestamp = artifact?.updatedAt?.takeIf { it.isNotBlank() } ?: modifiedAt

    Scaffold(
        snackbarHost = { SnackbarHost(snackbar) },
        bottomBar = {
            if (!path.startsWith("http://") && !path.startsWith("https://")) {
                Row(
                    Modifier
                        .fillMaxWidth()
                        .background(MaterialTheme.colorScheme.surfaceContainer)
                        .border(0.5.dp, MaterialTheme.colorScheme.outlineVariant)
                        .navigationBarsPadding()
                        .padding(horizontal = 16.dp, vertical = 10.dp),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    OutlinedButton(
                        onClick = {
                            clipboard.setText(AnnotatedString(path))
                            scope.launch { snackbar.showSnackbar(copiedMessage) }
                        },
                        modifier = Modifier.weight(1f).heightIn(min = 48.dp),
                    ) { Text(stringResource(Res.string.file_preview_copy_path)) }
                    Button(
                        onClick = {
                            scope.launch {
                                val file = bytes?.let {
                                    PickedFile(title.ifBlank { "artifact" }, mimeTypeFor(path, image != null), it)
                                }
                                val result = runCatching {
                                    if (path.startsWith("http://") || path.startsWith("https://")) {
                                        openExternalUrl(path)
                                    } else if (file != null) {
                                        // Android's document picker is the available system handoff for a local artifact.
                                        exportFile(file)
                                    } else false
                                }
                                if (result.isFailure || result.getOrNull() != true) {
                                    snackbar.showSnackbar(failedMessage.replace("%1\$s", result.exceptionOrNull()?.message ?: exportLabel))
                                }
                            }
                        },
                        modifier = Modifier.weight(1f).heightIn(min = 48.dp),
                        enabled = !loading && (bytes != null || path.startsWith("http://") || path.startsWith("https://")),
                    ) { Text(stringResource(Res.string.file_preview_share_open)) }
                }
            }
        },
    ) { insets ->
        Column(Modifier.fillMaxSize().padding(insets)) {
            CenterAlignedTopAppBar(
                title = {
                    Column(horizontalAlignment = Alignment.CenterHorizontally) {
                        Text(title, maxLines = 1, style = MaterialTheme.typography.titleLarge)
                        Text(
                            text = if (artifactAuthor.isBlank()) stringResource(Res.string.file_preview_author_unknown)
                            else stringResource(Res.string.feature_artifact_author, artifactAuthor),
                            style = MaterialTheme.typography.labelSmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                },
                navigationIcon = {
                    TextButton(onClick = onBack) { Text(stringResource(Res.string.common_back)) }
                },
                actions = {
                    Text(
                        stringResource(Res.string.file_preview_version_unknown),
                        modifier = Modifier.padding(end = 12.dp),
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                },
            )
            if (timestamp.isNotBlank()) {
                Text(
                    stringResource(Res.string.file_preview_modified, timestamp),
                    modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 2.dp),
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            Row(
                Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 6.dp)
                    .background(MaterialTheme.colorScheme.surfaceContainer, RoundedCornerShape(10.dp))
                    .padding(3.dp),
                horizontalArrangement = Arrangement.spacedBy(2.dp),
            ) {
                PreviewTab.entries.forEach { tab ->
                    val selected = selectedTab == tab
                    val label = when (tab) {
                        PreviewTab.Preview -> stringResource(Res.string.file_preview_tab_preview)
                        PreviewTab.Source -> stringResource(Res.string.file_preview_tab_source)
                        PreviewTab.History -> stringResource(Res.string.file_preview_tab_history)
                    }
                    Text(
                        text = label,
                        modifier = Modifier.weight(1f).heightIn(min = 40.dp)
                            .background(
                                if (selected) MaterialTheme.colorScheme.surface else MaterialTheme.colorScheme.surfaceContainer,
                                RoundedCornerShape(8.dp),
                            )
                            .clickable { selectedTab = tab }
                            .wrapContentHeight(Alignment.CenterVertically),
                        style = MaterialTheme.typography.labelLarge.copy(fontWeight = if (selected) FontWeight.SemiBold else FontWeight.Normal),
                        color = if (selected) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant,
                        textAlign = androidx.compose.ui.text.style.TextAlign.Center,
                    )
                }
            }
            if (loading) LinearProgressIndicator(Modifier.fillMaxWidth())
            error?.let {
                Text(it, Modifier.padding(16.dp), color = MaterialTheme.colorScheme.error)
            }
            when (selectedTab) {
                PreviewTab.History -> {
                    val artifactId = artifact?.id.orEmpty()
                    if (artifactId.isBlank()) {
                        Text(stringResource(Res.string.file_preview_history_unavailable), Modifier.padding(16.dp), color = MaterialTheme.colorScheme.onSurfaceVariant)
                    } else {
                        ArtifactHistory(
                            artifactId = artifactId,
                            versions = historyVersions,
                            hasMore = historyHasMore,
                            loading = historyLoading,
                            error = historyError,
                            selectedVersion = selectedVersion,
                            selectedContent = selectedVersionContent,
                            onLoadMore = {
                                historyVersions.lastOrNull()?.long("version")?.let { before ->
                                    historyLoading = true
                                    historyError = null
                                    scope.launch {
                                        runCatching {
                                            repository.call("artifact.versions.list", buildJsonObject {
                                                put("artifact_id", artifactId)
                                                put("limit", 20)
                                                put("before_version", before)
                                            })
                                        }.onSuccess { result ->
                                            historyVersions = historyVersions + result.objects("versions")
                                            historyHasMore = result.boolean("has_more")
                                        }.onFailure { historyError = it.message ?: "" }
                                        historyLoading = false
                                    }
                                }
                            },
                            onSelect = { version ->
                                selectedVersion = version.long("version")
                                selectedVersionContent = null
                                historyLoading = true
                                historyError = null
                                scope.launch {
                                    runCatching {
                                        repository.call("artifact.version.get", buildJsonObject {
                                            put("artifact_id", artifactId)
                                            put("version", version.long("version"))
                                        })
                                    }.onSuccess { result ->
                                        val encoded = result.str("content")
                                        runCatching {
                                            when (result.str("content_encoding")) {
                                                "base64" -> Base64.Default.decode(encoded)
                                                else -> encoded.encodeToByteArray()
                                            }
                                        }.onSuccess { selectedVersionContent = it }
                                            .onFailure { historyError = it.message ?: "" }
                                    }.onFailure { historyError = it.message ?: "" }
                                    historyLoading = false
                                }
                            },
                        )
                    }
                }
                PreviewTab.Source -> SourceContent(bytes)
                PreviewTab.Preview -> PreviewContent(bytes, image, isText, entries) { selectedPath ->
                    path = selectedPath
                    selectedTab = PreviewTab.Preview
                }
            }
        }
    }
}

@Composable
private fun ArtifactHistory(
    artifactId: String,
    versions: List<JsonObject>,
    hasMore: Boolean,
    loading: Boolean,
    error: String?,
    selectedVersion: Long?,
    selectedContent: ByteArray?,
    onLoadMore: () -> Unit,
    onSelect: (JsonObject) -> Unit,
) {
    Column(
        Modifier.fillMaxSize().padding(horizontal = 16.dp, vertical = 12.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        if (artifactId.isBlank()) Text(stringResource(Res.string.file_preview_history_unavailable), color = MaterialTheme.colorScheme.onSurfaceVariant)
        error?.takeIf(String::isNotBlank)?.let { Text(stringResource(Res.string.file_preview_history_error, it), color = MaterialTheme.colorScheme.error) }
        if (loading && versions.isEmpty()) LinearProgressIndicator(Modifier.fillMaxWidth())
        if (!loading && error == null && versions.isEmpty()) Text(stringResource(Res.string.file_preview_versions_empty), color = MaterialTheme.colorScheme.onSurfaceVariant)
        LazyColumn(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            items(versions, key = { "${it.str("artifact_id")}:${it.long("version")}" }) { version ->
                val number = version.long("version")
                ListItem(
                    headlineContent = { Text(stringResource(Res.string.file_preview_version_row, number.toInt(), version.str("created_at"))) },
                    supportingContent = {
                        Text(
                            listOf(version.str("title"), version.str("path_or_url"), version.str("bot_id")).filter(String::isNotBlank).joinToString(" · "),
                            maxLines = 1,
                        )
                    },
                    modifier = Modifier.clickable { onSelect(version) },
                    trailingContent = { if (selectedVersion == number) Text("✓", color = MaterialTheme.colorScheme.primary) },
                )
            }
            if (hasMore) item {
                TextButton(onClick = onLoadMore, enabled = !loading, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) {
                    Text(stringResource(Res.string.file_preview_more_versions))
                }
            }
        }
        if (loading && selectedVersion != null) LinearProgressIndicator(Modifier.fillMaxWidth())
        selectedContent?.let { bytes ->
            Surface(shape = RoundedCornerShape(12.dp), color = MaterialTheme.colorScheme.surfaceContainer) {
                Column(Modifier.fillMaxWidth().heightIn(max = 280.dp).verticalScroll(rememberScrollState()).padding(14.dp)) {
                    Text(stringResource(Res.string.file_preview_version_content, selectedVersion?.toInt() ?: 0), style = MaterialTheme.typography.titleMedium)
                    SelectionContainer {
                        Text(bytes.decodeToString().take(200_000), style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace))
                    }
                }
            }
        }
    }
}

@Composable
private fun SourceContent(bytes: ByteArray?) {
    if (bytes == null) {
        Text(stringResource(Res.string.file_preview_source_unavailable), Modifier.padding(16.dp), color = MaterialTheme.colorScheme.onSurfaceVariant)
    } else {
        SelectionContainer {
            LazyColumn(Modifier.fillMaxSize().padding(horizontal = 16.dp), contentPadding = PaddingValues(vertical = 12.dp)) {
                item {
                    Text(
                        bytes.decodeToString().take(200_000),
                        style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
                        color = MaterialTheme.colorScheme.onSurface,
                    )
                }
            }
        }
    }
}

@Composable
private fun PreviewContent(
    bytes: ByteArray?,
    image: androidx.compose.ui.graphics.ImageBitmap?,
    isText: Boolean,
    entries: List<JsonObject>,
    onOpen: (String) -> Unit,
) {
    if (entries.isNotEmpty()) {
        LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(horizontal = 16.dp, vertical = 8.dp)) {
            items(entries, key = { it.str("path") }) { entry ->
                ListItem(
                    headlineContent = { Text(entry.str("name"), maxLines = 1) },
                    supportingContent = { if (!entry.boolean("is_dir")) Text(entry.str("size")) },
                    trailingContent = { Text(if (entry.boolean("is_dir")) "›" else "▤") },
                    modifier = Modifier.clickable { onOpen(entry.str("path")) },
                )
            }
        }
    } else if (bytes == null) {
        Spacer(Modifier.fillMaxSize())
    } else if (image != null) {
        Image(image, contentDescription = null, Modifier.fillMaxSize().padding(12.dp), contentScale = ContentScale.Fit)
    } else if (isText) {
        LazyColumn(
            Modifier.fillMaxSize().padding(horizontal = 12.dp),
            contentPadding = PaddingValues(vertical = 12.dp),
        ) {
            item {
                Surface(
                    modifier = Modifier.fillMaxWidth(),
                    shape = RoundedCornerShape(12.dp),
                    color = MaterialTheme.colorScheme.surface,
                    border = androidx.compose.foundation.BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
                ) {
                    Column(Modifier.padding(18.dp)) { MarkdownText(bytes.decodeToString().take(200_000)) }
                }
            }
        }
    } else {
        Text(stringResource(Res.string.file_preview_source_unavailable), Modifier.padding(16.dp), color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

private fun mimeTypeFor(path: String, isImage: Boolean): String {
    if (isImage) return "image/*"
    return when (path.substringAfterLast('.', "").lowercase()) {
        "md", "markdown" -> "text/markdown"
        "txt", "log" -> "text/plain"
        "json" -> "application/json"
        "csv" -> "text/csv"
        "pdf" -> "application/pdf"
        else -> "application/octet-stream"
    }
}
