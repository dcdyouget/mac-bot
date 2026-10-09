package bot.mac.mobile.core.platform

import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.runtime.Composable

interface CredentialStore {
    suspend fun read(hostId: String): String?
    suspend fun write(hostId: String, password: String)
    suspend fun delete(hostId: String)
}

interface PersistentStore {
    suspend fun read(key: String): String?
    suspend fun write(key: String, value: String)
    suspend fun delete(key: String)

    suspend fun lastSeq(hostId: String): Long = read("last_seq:$hostId")?.toLongOrNull() ?: 0L
    suspend fun setLastSeq(hostId: String, seq: Long) {
        val key = "last_seq:$hostId"
        val current = read(key)?.toLongOrNull() ?: 0L
        if (seq >= current) write(key, seq.toString())
    }
    suspend fun hostRecordsJson(): String? = read("host_records")
    suspend fun setHostRecordsJson(json: String) { write("host_records", json) }
}

data class PickedFile(val name: String, val mimeType: String?, val bytes: ByteArray)

interface FilePicker { suspend fun pickFile(): PickedFile? }

interface ScreenImageDecoder { fun decodeJpeg(bytes: ByteArray): ImageBitmap? }

@Composable
expect fun PlatformBackHandler(enabled: Boolean = true, onBack: () -> Unit)

@Composable
expect fun LandscapeScreen(enabled: Boolean)

/** Installs Android-backed implementations before AppRuntime is created. */
expect fun initializePlatform(context: Any)
expect fun installFilePickerProvider(provider: suspend () -> PickedFile?)
expect fun installFileExporterProvider(provider: suspend (PickedFile) -> Boolean)
expect suspend fun exportFile(file: PickedFile): Boolean
expect suspend fun openExternalUrl(url: String): Boolean
expect fun platformCredentialStore(): CredentialStore
expect fun platformPersistentStore(): PersistentStore
expect fun platformFilePicker(): FilePicker
expect fun platformScreenImageDecoder(): ScreenImageDecoder
