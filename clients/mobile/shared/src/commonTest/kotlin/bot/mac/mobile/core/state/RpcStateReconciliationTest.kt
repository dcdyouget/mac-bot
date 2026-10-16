package bot.mac.mobile.core.state

import bot.mac.mobile.core.network.MainEvent
import bot.mac.mobile.core.protocol.protocolJson
import bot.mac.mobile.core.protocol.str
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonObject
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class RpcStateReconciliationTest {
    private fun obj(json: String): JsonObject = protocolJson.parseToJsonElement(json).jsonObject

    @Test fun initialEntityStillReceivesAnInterveningPartialStatusEvent() {
        val initial = MobileState()
        val fence = RpcStateFence(initial, 0)
        fence.record(MainEvent.Persistent(1, "bot.status", obj("""{"bot_id":"b","status":{"summary":"working"}}""")))
        val next = reconcileRpc(fence, initial.copy(lastSeq = 1), "bot.list", obj("""{"include_hidden":true}"""), obj("""{"bots":[{"id":"b","name":"loaded","status":{"summary":"idle"}}]}"""))
        assertEquals("loaded", next.bots.single().str("name"))
        assertEquals("working", next.bots.single()["status"]!!.jsonObject.str("summary"))
        assertEquals(1L, next.lastSeq)
    }

    @Test fun newerResponseEntitiesWinWithoutDroppingUntouchedInitialEntities() {
        val initial = MobileState()
        val current = initial.copy(bots = listOf(obj("""{"id":"b","name":"new response"}""")))
        val next = reconcileRpc(RpcStateFence(initial, 0), current, "bot.list", obj("""{"include_hidden":true}"""), obj("""{"bots":[{"id":"b","name":"old response"},{"id":"c","name":"initial load"}]}"""))
        assertEquals(listOf("new response", "initial load"), next.bots.map { it.str("name") })
    }

    @Test fun delayedHistoryCannotResurrectUnknownDeletedMessages() {
        val initial = MobileState()
        val fence = RpcStateFence(initial, 0)
        fence.record(MainEvent.Persistent(1, "message.deleted", obj("""{"chat_id":"chat","message_id":"gone"}""")))
        val current = StateReducer.event(initial, "message.deleted", obj("""{"chat_id":"chat","message_id":"gone"}"""), 1)
        val next = reconcileRpc(fence, current, "chat.history", obj("""{"chat_id":"chat"}"""), obj("""{"messages":[{"id":"gone","chat_id":"chat","seq":1},{"id":"kept","chat_id":"chat","seq":2}]}"""))
        assertEquals(listOf("kept"), next.messages["chat"]!!.map { it.str("id") })
    }

    @Test fun streamingDeltaIsNotAppliedTwiceWhenFinalResponseArrives() {
        val initial = MobileState(messages = mapOf("c" to listOf(obj("""{"id":"m","chat_id":"c","seq":1,"sender":{"kind":"bot"},"fallback_text":"a","streaming":true}"""))))
        val data = obj("""{"chat_id":"c","message_id":"m","text":"b"}""")
        val fence = RpcStateFence(initial, 0)
        fence.record(MainEvent.Ephemeral("message.delta", data))
        val current = StateReducer.event(initial, "message.delta", data)
        val next = reconcileRpc(fence, current, "chat.history", obj("""{"chat_id":"c"}"""), obj("""{"messages":[{"id":"m","chat_id":"c","seq":1,"fallback_text":"ab","streaming":false}]}"""))
        assertEquals("ab", next.messages["c"]!!.single().str("fallback_text"))
    }

    @Test fun requestJournalHasAFixedMemoryBound() {
        val fence = RpcStateFence(MobileState(), 0)
        repeat(5000) { fence.record(MainEvent.Ephemeral("typing", obj("{}"))) }
        assertTrue(fence.overflow)
        assertTrue(fence.events.isEmpty())
    }
}
