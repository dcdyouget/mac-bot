package bot.mac.mobile.core.platform

import kotlinx.coroutines.runBlocking
import org.junit.Before
import org.junit.Test
import org.robolectric.RuntimeEnvironment
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull

class AndroidPersistentStoreTest {
    @Before
    fun setUp() {
        initializePlatform(RuntimeEnvironment.getApplication())
    }

    @Test
    fun storesHostRecordsAndMonotonicSequence() = runBlocking {
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
