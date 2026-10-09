package bot.mac.mobile.core.state
import bot.mac.mobile.core.protocol.*
import kotlinx.serialization.json.*
import kotlin.test.*
class StateReducerTest {
    private fun objectOf(text: String) = protocolJson.parseToJsonElement(text).jsonObject
    @Test fun duplicateAndOldEventsDoNotOverwriteNewState() {
        val data = objectOf("""{"bot":{"id":"b","name":"new"}}""")
        val state = StateReducer.event(MobileState(), "bot.updated", data, 10)
        assertEquals(state, StateReducer.event(state, "bot.updated", objectOf("""{"bot":{"id":"b","name":"old"}}"""), 9))
        assertEquals(state, StateReducer.event(state, "bot.updated", data, 10))
        assertEquals("new", state.bots.single().str("name"))
    }
    @Test fun messageHistoryAndUpdateMergeByIdAndSortByChatSeq() {
        val one = objectOf("""{"id":"m1","chat_id":"c","seq":1,"fallback_text":"old"}""")
        val two = objectOf("""{"id":"m2","chat_id":"c","seq":2}""")
        val updated = objectOf("""{"id":"m1","chat_id":"c","seq":1,"fallback_text":"final","delivery":[{"state":"read"}]}""")
        val result = mergeMessages(listOf(two, one), listOf(updated))
        assertEquals(listOf("m1", "m2"), result.map { it.str("id") })
        assertEquals("final", result.first().str("fallback_text"))
        assertEquals("read", result.first().objects("delivery").single().str("state"))
    }
    @Test fun resetReplacesSnapshotWhileUnknownEventCommitsCursor() {
        val old = MobileState(bots = listOf(objectOf("""{"id":"old"}""")), messages = mapOf("c" to listOf(objectOf("""{"id":"m"}"""))))
        val reset = StateReducer.bootstrap(old, objectOf("""{"seq":70,"bots":[{"id":"new"}],"chats":[],"projects":[],"pending":{}}"""))
        assertEquals("new", reset.bots.single().str("id")); assertTrue(reset.messages.isEmpty())
        assertEquals(71L, StateReducer.event(reset, "future.event", buildJsonObject {}, 71).lastSeq)
    }
    @Test fun groupDeltasAreIgnoredAndFinalResponseReplacesFragments() {
        val current = MobileState(chats = listOf(objectOf("""{"id":"c","kind":"project"}""")), messages=mapOf("c" to listOf(objectOf("""{"id":"m","fallback_text":"atomic"}"""))))
        val unchanged = StateReducer.event(current,"message.delta",objectOf("""{"chat_id":"c","message_id":"m","text":"illegal"}"""))
        assertEquals(current, unchanged)
        val streaming=StateReducer.event(current,"trace.delta",objectOf("""{"stream":"s","request_id":"r","channel":"text","text":"partial"}"""))
        val final=StateReducer.event(streaming,"trace.item",objectOf("""{"stream":"s","item":{"assignment_id":"a","aseq":2,"type":"llm.response","data":{"request_id":"r","text":"full"}}}"""))
        assertTrue(final.traceFragments.isEmpty()); assertEquals("full",final.traces.getValue("a").single().obj("data").str("text"))
    }
    @Test fun privateDeltasUpdateVisibleBlocksAndFinalResponseReplacesPartialText() {
        val initial = MobileState(chats=listOf(objectOf("""{"id":"c","kind":"direct"}""")),messages=mapOf("c" to listOf(objectOf("""{"id":"m","chat_id":"c","seq":1,"blocks":[{"type":"text","markdown":"Hi"}],"fallback_text":"Hi","streaming":true}"""))))
        val partial=StateReducer.event(initial,"message.delta",objectOf("""{"chat_id":"c","message_id":"m","text":" there"}"""))
        assertEquals("Hi there",partial.messages.getValue("c").single().objects("blocks").single().str("markdown"))
        val final=StateReducer.event(partial,"message.updated",objectOf("""{"message":{"id":"m","chat_id":"c","seq":1,"blocks":[{"type":"text","markdown":"Hello"}],"fallback_text":"Hello","streaming":false}}"""),2)
        assertEquals("Hello",final.messages.getValue("c").single().objects("blocks").single().str("markdown"))
        assertFalse(final.messages.getValue("c").single().boolean("streaming"))
    }
    @Test fun traceCursorMergesOutOfOrderAndReplayedItems() {
        var current=MobileState()
        listOf(3,1,3,2).forEach { seq -> current=StateReducer.event(current,"trace.item",objectOf("""{"item":{"chat_id":"c","aseq":$seq,"type":"tool.start","data":{}}}""")) }
        assertEquals(listOf(1L,2L,3L),current.traces.getValue("c").map { it.long("aseq") })
    }
}
