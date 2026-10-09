package bot.mac.mobile

import android.Manifest
import android.content.res.Configuration
import android.net.Uri
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.result.ActivityResultLauncher
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalConfiguration
import androidx.core.view.WindowCompat
import bot.mac.mobile.core.platform.initializePlatform
import bot.mac.mobile.core.platform.installFilePickerProvider
import bot.mac.mobile.core.platform.installFileExporterProvider
import bot.mac.mobile.core.platform.PickedFile
import bot.mac.mobile.core.state.AppRuntime
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.CancellableContinuation
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
    private var fileContinuation: CancellableContinuation<PickedFile?>? = null
    private var exportFile: PickedFile? = null
    private var exportContinuation: CancellableContinuation<Boolean>? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        initializePlatform(applicationContext)
        if (savedInstanceState == null) MacBotDeepLinks.publish(intent)
        fileLauncher = registerForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
            val continuation = fileContinuation ?: return@registerForActivityResult
            if (uri == null) {
                fileContinuation = null
                continuation.resume(null)
            } else {
                lifecycleScope.launch {
                    val result = runCatching { readPickedFile(uri) }.getOrNull()
                    if (continuation.isActive) continuation.resume(result)
                    if (fileContinuation === continuation) fileContinuation = null
                }
            }
        }
        exportLauncher = registerForActivityResult(ActivityResultContracts.CreateDocument("application/octet-stream")) { uri ->
            val file = exportFile
            val continuation = exportContinuation
            if (file == null || uri == null) {
                exportFile = null
                exportContinuation = null
                continuation?.resume(false)
            } else {
                lifecycleScope.launch {
                    val result = runCatching { writeExportedFile(uri, file) }.getOrDefault(false)
                    if (continuation != null && continuation.isActive) continuation.resume(result)
                    if (exportContinuation === continuation) {
                        exportContinuation = null
                        exportFile = null
                    }
                }
            }
        }
        installFilePickerProvider { pickFile() }
        installFileExporterProvider { file -> export(file) }
        MacBotNotifications.ensureChannels(this)
        MainConnectionService.start(this)
        if (Build.VERSION.SDK_INT >= 33) notificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
        setContent {
            val theme by AppRuntime.repository.theme.collectAsState()
            val configuration = LocalConfiguration.current
            val systemDark = (configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK) ==
                Configuration.UI_MODE_NIGHT_YES
            val dark = theme == "dark" || theme == "system" && systemDark
            DisposableEffect(dark) {
                val barColor = if (dark) Color(0xFF1C1C1E) else Color(0xFFF7F7FA)
                window.statusBarColor = barColor.toArgb()
                window.navigationBarColor = barColor.toArgb()
                WindowCompat.getInsetsController(window, window.decorView).apply {
                    isAppearanceLightStatusBars = !dark
                    isAppearanceLightNavigationBars = !dark
                }
                onDispose { }
            }
            App()
        }
    }

    override fun onNewIntent(intent: android.content.Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        MacBotDeepLinks.publish(intent)
    }

    override fun onDestroy() {
        fileContinuation?.let { continuation ->
            fileContinuation = null
            if (continuation.isActive) continuation.resume(null)
        }
        exportContinuation?.let { continuation ->
            exportContinuation = null
            exportFile = null
            if (continuation.isActive) continuation.resume(false)
        }
        exportFile = null
        super.onDestroy()
    }

    private suspend fun pickFile(): PickedFile? = suspendCancellableCoroutine { continuation ->
        if (fileContinuation != null) {
            continuation.resume(null)
            return@suspendCancellableCoroutine
        }
        fileContinuation = continuation
        continuation.invokeOnCancellation {
            if (fileContinuation === continuation) fileContinuation = null
        }
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
        exportContinuation = continuation
        continuation.invokeOnCancellation {
            if (exportContinuation === continuation) {
                exportFile = null
                exportContinuation = null
            }
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
        intent.data = null
    }
}
