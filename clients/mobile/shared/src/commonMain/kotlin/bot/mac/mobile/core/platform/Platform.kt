package bot.mac.mobile.core.platform

import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.runtime.Composable
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.state.MobileState

interface CredentialStore {
    suspend fun read(hostId: String): String?
    suspend fun write(hostId: String, password: String)
    suspend fun delete(hostId: String)
}

interface PersistentStore {
    suspend fun read(key: String): String?
    suspend fun write(key: String, value: String)
    /** Snapshot writes may be coalesced by a platform without weakening config writes. */
    suspend fun writeSnapshot(key: String, value: String) = write(key, value)
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

enum class FilePickerMode { FILE, IMAGE, SCREENSHOT }

interface FilePicker { suspend fun pickFile(mode: FilePickerMode = FilePickerMode.FILE): PickedFile? }

interface ScreenImageDecoder { fun decodeJpeg(bytes: ByteArray): ImageBitmap? }

enum class DeepLinkKind(val authority: String) {
    APPROVAL("approval"),
    REVIEW("review"),
    CHAT("chat"),
    COMPUTER("computer"),
}

data class MobileDeepLink(
    val kind: DeepLinkKind,
    val resourceId: String,
    val hostId: String?,
    val messageId: String? = null,
)

/** Parses only supported macbot links; malformed or unknown routes are rejected. */
expect fun parseMobileDeepLink(value: String): MobileDeepLink?

fun MobileDeepLink.resolveHost(knownHostIds: Set<String>, activeHostId: String?): String? {
    val resolved = hostId ?: activeHostId ?: return null
    return resolved.takeIf { it in knownHostIds }
}

fun MobileState.containsDeepLinkResource(link: MobileDeepLink): Boolean = when (link.kind) {
    DeepLinkKind.APPROVAL -> approvals.any { it.str("id") == link.resourceId }
    DeepLinkKind.REVIEW -> projects.any { it.str("id") == link.resourceId }
    DeepLinkKind.CHAT -> chats.any { it.str("id") == link.resourceId }
    DeepLinkKind.COMPUTER -> bots.any { it.str("id") == link.resourceId }
}

@Composable
expect fun PlatformBackHandler(enabled: Boolean = true, onBack: () -> Unit)

@Composable
expect fun LandscapeScreen(enabled: Boolean)

/** Installs Android-backed implementations before AppRuntime is created. */
expect fun initializePlatform(context: Any)
expect fun installFilePickerProvider(provider: suspend (FilePickerMode) -> PickedFile?)
expect fun installFileExporterProvider(provider: suspend (PickedFile) -> Boolean)
expect suspend fun exportFile(file: PickedFile): Boolean
expect suspend fun openExternalUrl(url: String): Boolean
expect fun platformCredentialStore(): CredentialStore
expect fun platformPersistentStore(): PersistentStore
expect fun platformFilePicker(): FilePicker
expect fun platformScreenImageDecoder(): ScreenImageDecoder
/** Returns the current system locale as a BCP 47 language tag. */
expect fun platformLocaleTag(): String

/** False when the platform cannot blur content behind surfaces in this app window. */
expect fun platformBackdropBlurSupported(): Boolean

/** Mirrors the system animation preference, including Android's reduced-motion setting. */
@Composable
expect fun platformAnimationsEnabled(): Boolean
