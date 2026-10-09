package bot.mac.mobile

import android.content.Context
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.After
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class NotificationLedgerTest {
    private val context: Context = ApplicationProvider.getApplicationContext()
    private val hostId = "notification-ledger-test"

    @Before
    fun clearLedger() {
        removeTestEntries()
    }

    @After
    fun restoreLedger() {
        removeTestEntries()
    }

    private fun removeTestEntries() {
        context.getSharedPreferences("macbot_notification_ledger", Context.MODE_PRIVATE)
            .edit()
            .remove("_last_persistent_seq:$hostId")
            .remove("$hostId:message:msg-1")
            .commit()
    }

    @Test
    fun seedProvidesBaselineForAnEmptyLedger() {
        NotificationLedger.seed(context, hostId, 7L)

        assertFalse(NotificationLedger.acceptSeq(context, hostId, 7L))
        assertTrue(NotificationLedger.acceptSeq(context, hostId, 8L))
    }

    @Test
    fun seedDoesNotOverwriteAnEventAcceptedDuringStartup() {
        assertTrue(NotificationLedger.acceptSeq(context, hostId, 12L))

        NotificationLedger.seed(context, hostId, 20L)

        assertFalse(NotificationLedger.acceptSeq(context, hostId, 12L))
        assertTrue(NotificationLedger.acceptSeq(context, hostId, 13L))
    }

    @Test
    fun streamingMessagePlaceholderIsNotNotifiableAndFinalMessageIsDeduplicated() {
        assertFalse(shouldNotifyMessage(buildJsonObject { put("streaming", true) }))
        assertTrue(shouldNotifyMessage(buildJsonObject { put("streaming", false) }))
        assertTrue(NotificationLedger.accept(context, "$hostId:message:msg-1", null))
        assertFalse(NotificationLedger.accept(context, "$hostId:message:msg-1", null))
    }
}
