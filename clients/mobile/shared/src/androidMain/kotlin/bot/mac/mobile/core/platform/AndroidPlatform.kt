package bot.mac.mobile.core.platform

import android.content.Context
import android.database.ContentObserver
import android.content.Intent
import android.content.SharedPreferences
import android.animation.ValueAnimator
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.graphics.BitmapFactory
import android.net.Uri
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import java.util.Base64
import java.util.Locale
import java.util.concurrent.atomic.AtomicReference
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

private val contextRef = AtomicReference<Context?>()
private val filePickerProvider = AtomicReference<(suspend (FilePickerMode) -> PickedFile?)?>(null)
private val fileExporterProvider = AtomicReference<(suspend (PickedFile) -> Boolean)?>(null)
private const val CREDENTIAL_PREFS = "macbot_credentials"
private const val STATE_PREFS = "macbot_state"
private const val KEY_ALIAS = "macbot.credentials.v1"

actual fun initializePlatform(context: Any) {
    val appContext = context as? Context ?: error("initializePlatform expects an Android Context")
    contextRef.set(appContext.applicationContext)
}

actual fun installFilePickerProvider(provider: suspend (FilePickerMode) -> PickedFile?) {
    filePickerProvider.set(provider)
}

actual fun installFileExporterProvider(provider: suspend (PickedFile) -> Boolean) {
    fileExporterProvider.set(provider)
}

actual suspend fun exportFile(file: PickedFile): Boolean = fileExporterProvider.get()?.invoke(file) ?: false

actual suspend fun openExternalUrl(url: String): Boolean = withContext(Dispatchers.Main.immediate) {
    val uri = runCatching { Uri.parse(url) }.getOrNull() ?: return@withContext false
    if (uri.scheme != "http" && uri.scheme != "https") return@withContext false
    val intent = Intent(Intent.ACTION_VIEW, uri).apply {
        addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    }
    runCatching {
        require(intent.resolveActivity(requireContext().packageManager) != null)
        requireContext().startActivity(intent)
        true
    }.getOrDefault(false)
}

private fun requireContext(): Context = contextRef.get() ?: error("Platform has not been initialized")

actual fun platformCredentialStore(): CredentialStore = AndroidCredentialStore(requireContext())
actual fun platformPersistentStore(): PersistentStore = AndroidPersistentStore(requireContext())
actual fun platformFilePicker(): FilePicker = AndroidFilePicker()
actual fun platformScreenImageDecoder(): ScreenImageDecoder = AndroidScreenImageDecoder()
actual fun platformLocaleTag(): String = Locale.getDefault().toLanguageTag()

private val deepLinkResourceIdPattern = Regex("[A-Za-z0-9][A-Za-z0-9_-]{0,127}")

actual fun parseMobileDeepLink(value: String): MobileDeepLink? {
    if (value.length !in 1..512) return null
    val uri = runCatching { Uri.parse(value) }.getOrNull() ?: return null
    if (!uri.isHierarchical || uri.scheme != "macbot" || uri.fragment != null || uri.port != -1) return null

    val kind = DeepLinkKind.entries.firstOrNull { it.authority == uri.host } ?: return null
    if (uri.encodedAuthority != kind.authority) return null

    val encodedPath = uri.encodedPath ?: return null
    if (!encodedPath.startsWith('/') || encodedPath.count { it == '/' } != 1) return null
    val resourceId = uri.pathSegments.singleOrNull() ?: return null
    if (!deepLinkResourceIdPattern.matches(resourceId)) return null

    val encodedQuery = uri.encodedQuery
    if (encodedQuery != null && (encodedQuery.isEmpty() || uri.queryParameterNames != setOf("host_id"))) return null
    val hostValues = uri.getQueryParameters("host_id")
    if (hostValues.size > 1) return null
    val hostId = hostValues.singleOrNull()
    if (hostId != null && !deepLinkResourceIdPattern.matches(hostId)) return null

    return MobileDeepLink(kind, resourceId, hostId)
}

// Android blur-behind APIs only blur other windows; they cannot blur surfaces
// within this single Activity, so design tokens require the opaque fallback.
actual fun platformBackdropBlurSupported(): Boolean = false

@Composable
actual fun platformAnimationsEnabled(): Boolean {
    val context = LocalContext.current
    var enabled by remember(context) { mutableStateOf(ValueAnimator.areAnimatorsEnabled()) }
    DisposableEffect(context) {
        val resolver = context.contentResolver
        val observer = object : ContentObserver(Handler(Looper.getMainLooper())) {
            override fun onChange(selfChange: Boolean) {
                enabled = ValueAnimator.areAnimatorsEnabled()
            }
        }
        resolver.registerContentObserver(Settings.Global.getUriFor(Settings.Global.ANIMATOR_DURATION_SCALE), false, observer)
        onDispose { resolver.unregisterContentObserver(observer) }
    }
    return enabled
}

private class AndroidCredentialStore(context: Context) : CredentialStore {
    private val prefs = context.getSharedPreferences(CREDENTIAL_PREFS, Context.MODE_PRIVATE)

    override suspend fun read(hostId: String): String? = withContext(Dispatchers.IO) {
        val encoded = prefs.getString(hostId, null) ?: return@withContext null
        runCatching {
            val packed = Base64.getDecoder().decode(encoded)
            require(packed.size > 12) { "Invalid credential payload" }
            val iv = packed.copyOfRange(0, 12)
            val ciphertext = packed.copyOfRange(12, packed.size)
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, iv))
            cipher.doFinal(ciphertext).toString(Charsets.UTF_8)
        }.getOrNull()
    }

    override suspend fun write(hostId: String, password: String) = withContext(Dispatchers.IO) {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        // AndroidKeyStore AES keys require randomized encryption. Let the
        // provider choose the IV, then persist cipher.iv with the ciphertext.
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val iv = cipher.iv
        val ciphertext = cipher.doFinal(password.toByteArray(Charsets.UTF_8))
        check(prefs.edit().putString(hostId, Base64.getEncoder().encodeToString(iv + ciphertext)).commit()) {
            "Unable to persist credential"
        }
    }

    override suspend fun delete(hostId: String) = withContext(Dispatchers.IO) {
        check(prefs.edit().remove(hostId).commit()) { "Unable to delete credential" }
    }

    private fun key(): SecretKey {
        val keyStore = java.security.KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (keyStore.getKey(KEY_ALIAS, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        generator.init(
            KeyGenParameterSpec.Builder(
                KEY_ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
            ).setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setUserAuthenticationRequired(false)
                .build(),
        )
        return generator.generateKey()
    }
}

private class AndroidPersistentStore(context: Context) : PersistentStore {
    private val prefs: SharedPreferences = context.getSharedPreferences(STATE_PREFS, Context.MODE_PRIVATE)
    private val sequenceLock = Any()
    override suspend fun read(key: String): String? = withContext(Dispatchers.IO) { prefs.getString(key, null) }
    override suspend fun write(key: String, value: String) = withContext(Dispatchers.IO) {
        check(prefs.edit().putString(key, value).commit()) { "Unable to persist state" }
    }
    override suspend fun writeSnapshot(key: String, value: String) = withContext(Dispatchers.IO) {
        // ClientRepository coalesces these writes. apply() updates the
        // in-memory view immediately; the following cursor commit waits for
        // pending applies before recording a cursor for this snapshot.
        prefs.edit().putString(key, value).apply()
    }
    override suspend fun delete(key: String) = withContext(Dispatchers.IO) {
        check(prefs.edit().remove(key).commit()) { "Unable to delete state" }
    }
    override suspend fun setLastSeq(hostId: String, seq: Long) = withContext(Dispatchers.IO) {
        synchronized(sequenceLock) {
            val key = "last_seq:$hostId"
            val current = prefs.getString(key, null)?.toLongOrNull() ?: 0L
            if (seq >= current) {
                check(prefs.edit().putString(key, seq.toString()).commit()) { "Unable to persist sequence" }
            }
        }
    }
}

/** The Activity may replace this with an ActivityResult-backed picker later. */
private class AndroidFilePicker : FilePicker {
    override suspend fun pickFile(mode: FilePickerMode): PickedFile? = filePickerProvider.get()?.invoke(mode)
}

private class AndroidScreenImageDecoder : ScreenImageDecoder {
    override fun decodeJpeg(bytes: ByteArray): ImageBitmap? =
        BitmapFactory.decodeByteArray(bytes, 0, bytes.size)?.asImageBitmap()
}
