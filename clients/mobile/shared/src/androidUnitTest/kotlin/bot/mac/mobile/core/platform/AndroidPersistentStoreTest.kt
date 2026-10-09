package bot.mac.mobile.core.platform

import kotlinx.coroutines.runBlocking
import org.junit.Before
import org.junit.Test
import org.robolectric.RuntimeEnvironment
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [36])
class AndroidPersistentStoreTest {
    @Before
    fun setUp() {
        initializePlatform(RuntimeEnvironment.getApplication())
    }

    @Test
    fun storesHostRecordsAndMonotonicSequence(): Unit = runBlocking {
        val store = platformPersistentStore()
        store.delete("host_records")
        store.delete("last_seq:test-host")
        assertNull(store.hostRecordsJson())
        store.setHostRecordsJson("[{\"id\":\"test-host\"}]")
        assertEquals("[{\"id\":\"test-host\"}]", store.hostRecordsJson())

        store.setLastSeq("test-host", 41)
        store.setLastSeq("test-host", 40)
        assertEquals(41L, store.lastSeq("test-host"))
    }
}
