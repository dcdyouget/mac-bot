package bot.mac.mobile

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import bot.mac.mobile.core.platform.initializePlatform
import bot.mac.mobile.core.platform.platformCredentialStore
import kotlinx.coroutines.runBlocking
import java.util.Base64
import org.junit.Test
import org.junit.runner.RunWith
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull

/** Runs on a device to exercise the app-private encrypted credential vault. */
@RunWith(AndroidJUnit4::class)
class AndroidCredentialStoreInstrumentedTest {
    @Test
    fun appPrivateVaultStoresCiphertextAndRoundTrips() = runBlocking {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        initializePlatform(context)
        val store = platformCredentialStore()
        val hostId = "instrumented-roundtrip-host"
        val password = "secret-password-for-roundtrip"
        store.write(hostId, password)

        val credential = context.getSharedPreferences("macbot_credentials", 0).getString(hostId, null)
        val key = context.getSharedPreferences("macbot_credential_vault", 0).getString("aes_key_v1", null)
        assertNotNull(credential)
        assertFalse(requireNotNull(credential).contains(password))
        assertNotNull(key)
        assertEquals(password, store.read(hostId))
        store.delete(hostId)
    }

    @Test
    fun corruptedCiphertextReturnsNull() = runBlocking {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        initializePlatform(context)
        val store = platformCredentialStore()
        val hostId = "instrumented-corrupt-host"
        store.write(hostId, "secret-password")

        val prefs = context.getSharedPreferences("macbot_credentials", 0)
        val packed = Base64.getDecoder().decode(requireNotNull(prefs.getString(hostId, null)))
        packed[packed.lastIndex] = (packed.last().toInt() xor 1).toByte()
        check(prefs.edit().putString(hostId, Base64.getEncoder().encodeToString(packed)).commit())

        assertNull(store.read(hostId))
        store.delete(hostId)
    }

    @Test
    fun deleteRemovesCredential() = runBlocking {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        initializePlatform(context)
        val store = platformCredentialStore()
        val hostId = "instrumented-delete-host"
        store.write(hostId, "secret-password")

        store.delete(hostId)

        assertNull(context.getSharedPreferences("macbot_credentials", 0).getString(hostId, null))
        assertNull(store.read(hostId))
    }
}
