package bot.mac.mobile

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import bot.mac.mobile.core.platform.initializePlatform
import bot.mac.mobile.core.platform.platformCredentialStore
import kotlinx.coroutines.runBlocking
import org.junit.Test
import org.junit.runner.RunWith
import org.junit.Assert.assertEquals

/** Runs on an emulator/device because Robolectric does not provide AndroidKeyStore. */
@RunWith(AndroidJUnit4::class)
class AndroidCredentialStoreInstrumentedTest {
    @Test
    fun keystoreRoundTrip() = runBlocking {
        initializePlatform(InstrumentationRegistry.getInstrumentation().targetContext)
        val store = platformCredentialStore()
        store.write("instrumented-host", "secret-password")
        assertEquals("secret-password", store.read("instrumented-host"))
        store.delete("instrumented-host")
    }
}
