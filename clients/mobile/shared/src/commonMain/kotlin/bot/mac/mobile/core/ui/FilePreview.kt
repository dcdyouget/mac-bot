package bot.mac.mobile.core.ui

import androidx.compose.foundation.Image
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.*
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.platform.*
import bot.mac.mobile.core.protocol.*
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.resources.*
import kotlinx.coroutines.launch
import kotlinx.serialization.json.*
import org.jetbrains.compose.resources.stringResource

@Composable fun FilePreview(repository: MobileRepository, rootId: String, initialPath: String, onBack: () -> Unit) {
    var path by remember(initialPath) { mutableStateOf(initialPath) }
    var content by remember { mutableStateOf<ByteArray?>(null) }
    var entries by remember { mutableStateOf<List<JsonObject>>(emptyList()) }
    var error by remember { mutableStateOf<String?>(null) }
    var loading by remember { mutableStateOf(true) }
    val scope = rememberCoroutineScope()
    LaunchedEffect(rootId,path) {
        loading = true; content = null; error = null; entries = emptyList()
        if (path.startsWith("http://") || path.startsWith("https://")) {
            runCatching { openExternalUrl(path) }.onFailure { error=it.message }; loading=false
        } else {
            val params = mapOf("root" to "project", "root_id" to rootId, "path" to path)
            runCatching { repository.fetchBytes("/api/v1/files", params) }.onSuccess { content=it }.onFailure {
                runCatching { protocolJson.parseToJsonElement(repository.fetchText("/api/v1/files/list",params)).jsonObject.objects("entries") }.onSuccess { values-> entries=values }.onFailure { failure->error=failure.message }
            }
            loading=false
        }
    }
    val bytes=content
    val image = remember(bytes) { bytes?.let { platformScreenImageDecoder().decodeJpeg(it) } }
    Column(Modifier.fillMaxSize().padding(16.dp)) {
        Row { TextButton(onClick=onBack){Text(stringResource(Res.string.common_back))}; Text(path,Modifier.weight(1f).padding(12.dp),maxLines=2) }
        if(loading) LinearProgressIndicator(Modifier.fillMaxWidth())
        error?.let { Text(it,color=MaterialTheme.colorScheme.error) }
        if(bytes!=null) {
            Button(onClick={scope.launch{ runCatching { exportFile(PickedFile(path.substringAfterLast('/').ifBlank{"artifact"}, if(image!=null)"image/png" else "application/octet-stream",bytes)) }.onFailure { error=it.message } }}){Text(stringResource(Res.string.file_save))}
            if(image!=null) Image(image,null,Modifier.weight(1f).fillMaxWidth(),contentScale=ContentScale.Fit)
            else LazyColumn(Modifier.weight(1f)) { item { SelectionContainer { Text(bytes.decodeToString().take(100000)) } } }
        }
        LazyColumn { items(entries,key={it.str("path")}) { entry -> ListItem(headlineContent={Text(entry.str("name"))},trailingContent={Text(if(entry.boolean("is_dir"))"›" else "▤")},modifier=Modifier.clickable { path=entry.str("path") }) } }
    }
}
