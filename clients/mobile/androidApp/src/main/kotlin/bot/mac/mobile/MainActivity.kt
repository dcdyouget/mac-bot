package bot.mac.mobile

import android.Manifest
import android.net.Uri
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.result.ActivityResultLauncher
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import bot.mac.mobile.core.platform.initializePlatform
import bot.mac.mobile.core.platform.installFilePickerProvider
import bot.mac.mobile.core.platform.installFileExporterProvider
import bot.mac.mobile.core.platform.PickedFile
import bot.mac.mobile.core.state.AppRuntime
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlin.coroutines.resume

class MainActivity : ComponentActivity() {
    private val notificationPermission =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) { }
    private lateinit var fileLauncher: ActivityResultLauncher<Array<String>>
    private lateinit var exportLauncher: ActivityResultLauncher<String>
    private var fileContinuation: ((PickedFile?) -> Unit)? = null
    private var exportFile: PickedFile? = null
    private var exportContinuation: ((Boolean) -> Unit)? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        initializePlatform(applicationContext)
        MacBotDeepLinks.publish(intent)
        fileLauncher = registerForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
            val continuation = fileContinuation ?: return@registerForActivityResult
            fileContinuation = null
            if (uri == null) {
                continuation(null)
            } else {
                lifecycleScope.launch {
                    continuation(readPickedFile(uri))
                }
            }
        }
        exportLauncher = registerForActivityResult(ActivityResultContracts.CreateDocument("application/octet-stream")) { uri ->
            val file = exportFile
            val continuation = exportContinuation
            exportFile = null
            exportContinuation = null
            if (file == null || uri == null) {
                continuation?.invoke(false)
            } else {
                lifecycleScope.launch {
                    continuation?.invoke(writeExportedFile(uri, file))
                }
            }
        }
        installFilePickerProvider { pickFile() }
        installFileExporterProvider { file -> export(file) }
        MacBotNotifications.ensureChannels(this)
        MainConnectionService.start(this)
        if (Build.VERSION.SDK_INT >= 33) notificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
        setContent { App() }
    }

    override fun onNewIntent(intent: android.content.Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        MacBotDeepLinks.publish(intent)
    }

    private suspend fun pickFile(): PickedFile? = suspendCancellableCoroutine { continuation ->
        fileContinuation = { result ->
            if (continuation.isActive) continuation.resume(result)
        }
        continuation.invokeOnCancellation { fileContinuation = null }
        lifecycleScope.launch(Dispatchers.Main.immediate) {
            if (continuation.isActive) fileLauncher.launch(arrayOf("*/*"))
        }
    }

    private suspend fun readPickedFile(uri: Uri): PickedFile? = withContext(Dispatchers.IO) {
        val metadata = contentResolver.query(
            uri,
            arrayOf(android.provider.OpenableColumns.DISPLAY_NAME, android.provider.OpenableColumns.SIZE),
            null,
            null,
            null,
        )
            ?.use { cursor ->
                if (cursor.moveToFirst()) {
                    val name = cursor.getString(0) ?: "attachment"
                    val size = if (!cursor.isNull(1)) cursor.getLong(1) else -1L
                    name to size
                } else null
            } ?: ("attachment" to -1L)
        if (metadata.second > MAX_ATTACHMENT_BYTES) return@withContext null
        val bytes = contentResolver.openInputStream(uri)?.use { input ->
            val output = java.io.ByteArrayOutputStream()
            val buffer = ByteArray(DEFAULT_BUFFER_SIZE)
            var total = 0L
            while (true) {
                val read = input.read(buffer)
                if (read < 0) break
                total += read
                if (total > MAX_ATTACHMENT_BYTES) return@use null
                output.write(buffer, 0, read)
            }
            output.toByteArray()
        } ?: return@withContext null
        PickedFile(metadata.first, contentResolver.getType(uri), bytes)
    }

    private suspend fun export(file: PickedFile): Boolean = suspendCancellableCoroutine { continuation ->
        if (exportFile != null || exportContinuation != null) {
            continuation.resume(false)
            return@suspendCancellableCoroutine
        }
        exportFile = file
        exportContinuation = { result ->
            if (continuation.isActive) continuation.resume(result)
        }
        continuation.invokeOnCancellation {
            exportFile = null
            exportContinuation = null
        }
        lifecycleScope.launch(Dispatchers.Main.immediate) {
            if (continuation.isActive) {
                exportLauncher.launch(file.name)
            }
        }
    }

    private suspend fun writeExportedFile(uri: Uri, file: PickedFile): Boolean = withContext(Dispatchers.IO) {
        runCatching {
            contentResolver.openOutputStream(uri)?.use { output -> output.write(file.bytes) } ?: error("Unable to open export destination")
            true
        }.getOrDefault(false)
    }

    private companion object { const val MAX_ATTACHMENT_BYTES = 100L * 1024L * 1024L }
}

object MacBotDeepLinks {
    fun publish(intent: android.content.Intent?) {
        val uri = intent?.data ?: return
        AppRuntime.repository.deepLink.value = uri.toString()
    }
}
