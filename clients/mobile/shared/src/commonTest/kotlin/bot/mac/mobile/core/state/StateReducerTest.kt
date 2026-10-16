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
    @Test fun pagedHistoryAndOutOfOrderMessageEventsRemainInCanonicalSequenceOrder() {
        fun message(id: String, seq: Int, timestamp: String) = objectOf(
            """{"id":"$id","chat_id":"c","seq":$seq,"created_at":"$timestamp"}""",
        )
        val historyPage = listOf(
            message("m3", 3, "2026-10-15T10:00:00Z"),
            message("m1", 1, "2026-10-15T09:00:00Z"),
        )
        val olderPage = listOf(message("m2", 2, "2026-10-14T23:59:00Z"), message("m1", 1, "2026-10-15T09:00:00Z"))
        var state = RpcStateReducer.apply(MobileState(), "chat.history", objectOf("""{"chat_id":"c"}"""), objectOf(
            """{"messages":[${historyPage.joinToString(",")}]}""",
        ))
        state = RpcStateReducer.apply(state, "chat.history", objectOf("""{"chat_id":"c"}"""), objectOf(
            """{"messages":[${olderPage.joinToString(",")}]}""",
        ))
        state = StateReducer.event(state, "message.created", objectOf("""{"message":{"id":"m4","chat_id":"c","seq":4,"created_at":"2026-10-15T10:01:00Z"}}"""), 4)

        assertEquals(listOf("m1", "m2", "m3", "m4"), state.messages.getValue("c").map { it.str("id") })
        assertEquals(listOf(1L, 2L, 3L, 4L), state.messages.getValue("c").map { it.long("seq") })
    }
    @Test fun loadedSkillsFromMessageUpdatedReplaceHistoryAndAreNotInferredFromDeliveryRead() {
        val history = objectOf("""{"id":"m1","chat_id":"c","seq":1,"sender":{"kind":"user"},"delivery":[{"bot_id":"b","state":"read"}]}""")
        val oldServerUpdate = objectOf("""{"message":{"id":"m1","chat_id":"c","seq":1,"sender":{"kind":"user"},"delivery":[{"bot_id":"b","state":"read"}]}}""")
        val newServerUpdate = objectOf("""{"message":{"id":"m1","chat_id":"c","seq":1,"sender":{"kind":"user"},"delivery":[{"bot_id":"b","state":"read"}],"loaded_skills":[{"name":"web-auth-patterns","bot_id":"b","assignment_id":"a","loaded_at":"2026-10-12T09:00:00Z"}]}}""")

        val initial = MobileState(messages = mapOf("c" to listOf(history)))
        val oldCompatible = StateReducer.event(initial, "message.updated", oldServerUpdate, 1)
        assertEquals(emptyList(), Message(oldCompatible.messages.getValue("c").single()).loadedSkills)

        val loaded = StateReducer.event(oldCompatible, "message.updated", newServerUpdate, 2)
        assertEquals("web-auth-patterns", Message(loaded.messages.getValue("c").single()).loadedSkills.single().name)
    }
    @Test fun resetReplacesSnapshotWhileUnknownEventCommitsCursor() {
        val old = MobileState(bots = listOf(objectOf("""{"id":"old"}""")), messages = mapOf("c" to listOf(objectOf("""{"id":"m"}"""))))
        val reset = StateReducer.bootstrap(old, objectOf("""{"seq":70,"bots":[{"id":"new"}],"chats":[],"projects":[],"pending":{}}"""))
        assertEquals("new", reset.bots.single().str("id")); assertTrue(reset.messages.isEmpty())
        assertEquals(71L, StateReducer.event(reset, "future.event", buildJsonObject {}, 71).lastSeq)
    }
    @Test fun projectStreamingPlaceholderMergesDeltaAndFinalReplayByMessageId() {
        val current = MobileState(chats = listOf(objectOf("""{"id":"group","kind":"project"}""")))
        val placeholder = objectOf("""{"id":"msgA","chat_id":"group","seq":1,"sender":{"kind":"bot","bot_id":"worker"},"blocks":[],"fallback_text":"","streaming":true}""")
        var state = StateReducer.event(current, "message.created", objectOf("""{"message":${placeholder}}"""), 1)
        assertEquals(listOf("msgA"), state.messages.getValue("group").map { it.str("id") })
        assertTrue(state.messages.getValue("group").single().boolean("streaming"))

        state = StateReducer.event(state, "message.delta", objectOf("""{"chat_id":"group","message_id":"msgA","text":"First"}"""))
        assertEquals("First", state.messages.getValue("group").single().str("fallback_text"))
        assertEquals("First", state.messages.getValue("group").single().objects("blocks").single().str("markdown"))

        val interim = objectOf("""{"message":{"id":"msgA","chat_id":"group","seq":1,"sender":{"kind":"bot","bot_id":"worker"},"blocks":[{"type":"text","markdown":"First reply"}],"fallback_text":"First reply","streaming":true}}""")
        state = StateReducer.event(state, "message.updated", interim, 2)
        assertEquals(listOf("msgA"), state.messages.getValue("group").map { it.str("id") })
        assertEquals("First reply", state.messages.getValue("group").single().str("fallback_text"))

        val final = objectOf("""{"message":{"id":"msgA","chat_id":"group","seq":1,"sender":{"kind":"bot","bot_id":"worker"},"blocks":[{"type":"text","markdown":"Complete"}],"fallback_text":"Complete","streaming":false}}""")
        state = StateReducer.event(state, "message.updated", final, 3)
        state = StateReducer.event(state, "message.updated", final)
        assertEquals(listOf("msgA"), state.messages.getValue("group").map { it.str("id") })
        assertEquals("Complete", state.messages.getValue("group").single().str("fallback_text"))
        assertFalse(state.messages.getValue("group").single().boolean("streaming"))

        val lateDelta = StateReducer.event(state, "message.delta", objectOf("""{"chat_id":"group","message_id":"msgA","text":"late"}"""))
        assertEquals(state, lateDelta)

        val nextMessage = objectOf("""{"message":{"id":"msgB","chat_id":"group","seq":2,"sender":{"kind":"bot","bot_id":"worker"},"blocks":[{"type":"text","markdown":"Next"}],"fallback_text":"Next","streaming":false}}""")
        state = StateReducer.event(state, "message.created", nextMessage, 4)
        assertEquals(listOf("msgA", "msgB"), state.messages.getValue("group").map { it.str("id") })

        val streaming=StateReducer.event(state,"trace.delta",objectOf("""{"stream":"s","request_id":"r","channel":"text","text":"partial"}"""))
        val traceFinal=StateReducer.event(streaming,"trace.item",objectOf("""{"stream":"s","item":{"assignment_id":"a","aseq":2,"type":"llm.response","data":{"request_id":"r","text":"full"}}}"""))
        assertTrue(traceFinal.traceFragments.isEmpty()); assertEquals("full",traceFinal.traces.getValue("a").single().obj("data").str("text"))
    }
    @Test fun deltasRequireMatchingChatAndStreamingMessage() {
        val current = MobileState(
            chats = listOf(objectOf("""{"id":"chat-a","kind":"direct"}"""), objectOf("""{"id":"chat-b","kind":"project"}""")),
            messages = mapOf(
                "chat-a" to listOf(objectOf("""{"id":"same-id","chat_id":"chat-a","sender":{"kind":"bot"},"fallback_text":"A","streaming":true}""")),
                "chat-b" to listOf(objectOf("""{"id":"same-id","chat_id":"chat-a","sender":{"kind":"bot"},"fallback_text":"corrupt","streaming":true}""")),
            ),
        )
        val wrongSession = StateReducer.event(current, "message.delta", objectOf("""{"chat_id":"chat-b","message_id":"same-id","text":"wrong"}"""))
        assertEquals(current, wrongSession)
        val valid = StateReducer.event(current, "message.delta", objectOf("""{"chat_id":"chat-a","message_id":"same-id","text":"!"}"""))
        assertEquals("A!", valid.messages.getValue("chat-a").single().str("fallback_text"))
        assertEquals("corrupt", valid.messages.getValue("chat-b").single().str("fallback_text"))

        val terminal = current.copy(messages = current.messages + ("chat-a" to listOf(objectOf("""{"id":"same-id","chat_id":"chat-a","sender":{"kind":"bot"},"fallback_text":"final","streaming":false}"""))))
        assertEquals(terminal, StateReducer.event(terminal, "message.delta", objectOf("""{"chat_id":"chat-a","message_id":"same-id","text":"late"}""")))

        val userMessage = current.copy(messages = current.messages + ("chat-a" to listOf(objectOf("""{"id":"same-id","chat_id":"chat-a","sender":{"kind":"user"},"fallback_text":"user","streaming":true}"""))))
        assertEquals(userMessage, StateReducer.event(userMessage, "message.delta", objectOf("""{"chat_id":"chat-a","message_id":"same-id","text":"wrong sender"}""")))
        val deletedMessage = current.copy(messages = current.messages + ("chat-a" to listOf(objectOf("""{"id":"same-id","chat_id":"chat-a","sender":{"kind":"bot"},"deleted":true,"fallback_text":"removed","streaming":true}"""))))
        assertEquals(deletedMessage, StateReducer.event(deletedMessage, "message.delta", objectOf("""{"chat_id":"chat-a","message_id":"same-id","text":"resurrect"}""")))
    }
    @Test fun privateDeltasUpdateVisibleBlocksAndFinalResponseReplacesPartialText() {
        val initial = MobileState(chats=listOf(objectOf("""{"id":"c","kind":"direct"}""")),messages=mapOf("c" to listOf(objectOf("""{"id":"m","chat_id":"c","seq":1,"sender":{"kind":"bot"},"blocks":[{"type":"text","markdown":"Hi"}],"fallback_text":"Hi","streaming":true}"""))))
        val partial=StateReducer.event(initial,"message.delta",objectOf("""{"chat_id":"c","message_id":"m","text":" there"}"""))
        assertEquals("Hi there",partial.messages.getValue("c").single().objects("blocks").single().str("markdown"))
        val final=StateReducer.event(partial,"message.updated",objectOf("""{"message":{"id":"m","chat_id":"c","seq":1,"blocks":[{"type":"text","markdown":"Hello"}],"fallback_text":"Hello","streaming":false}}"""),2)
        assertEquals("Hello",final.messages.getValue("c").single().objects("blocks").single().str("markdown"))
        assertFalse(final.messages.getValue("c").single().boolean("streaming"))
    }
    @Test fun typingStateIsRemovedOnStopTerminalBotMessageAndBootstrap() {
        val initial = MobileState(chats = listOf(objectOf("""{"id":"c","kind":"direct"}""")))
        val typing = StateReducer.event(initial, "typing", objectOf("""{"chat_id":"c","on":true}"""))
        assertEquals(true, typing.typing["c"])

        val stopped = StateReducer.event(typing, "typing", objectOf("""{"chat_id":"c","on":false}"""))
        assertFalse("c" in stopped.typing)

        val typingAgain = StateReducer.event(typing, "message.created", objectOf("""{"message":{"id":"m","chat_id":"c","seq":1,"sender":{"kind":"bot"},"streaming":false}}"""), 1)
        assertFalse("c" in typingAgain.typing)

        val reset = StateReducer.bootstrap(typing, objectOf("""{"seq":2,"bots":[],"chats":[],"projects":[],"pending":{}}"""))
        assertTrue(reset.typing.isEmpty())
    }
    @Test fun traceCursorMergesOutOfOrderAndReplayedItems() {
        var current=MobileState()
        listOf(3,1,3,2).forEach { seq -> current=StateReducer.event(current,"trace.item",objectOf("""{"item":{"chat_id":"c","aseq":$seq,"type":"tool.start","data":{}}}""")) }
        assertEquals(listOf(1L,2L,3L),current.traces.getValue("c").map { it.long("aseq") })
    }
    @Test fun memoryEventsSynchronizeEntryQuotaAndUserProfile() {
        val updated = StateReducer.event(
            MobileState(),
            "memory.user.updated",
            objectOf("""{"entry":{"id":"m1","content":"new","version":2},"quota":{"used_chars":3,"limit_chars":1500}}"""),
            1,
        )
        assertEquals("new", updated.memoryEntries.single().str("content"))
        assertEquals(3L, updated.memoryQuota.long("used_chars"))

        val profile = StateReducer.event(
            updated,
            "memory.profile.updated",
            objectOf("""{"profile":{"name":"Lin","timezone":"Asia/Shanghai","version":4}}"""),
            2,
        )
        assertEquals("Lin", profile.userProfile.str("name"))

        val deleted = StateReducer.event(
            profile,
            "memory.user.deleted",
            objectOf("""{"entry_id":"m1","quota":{"used_chars":0,"limit_chars":1500}}"""),
            3,
        )
        assertTrue(deleted.memoryEntries.isEmpty())
        assertEquals(0L, deleted.memoryQuota.long("used_chars"))
    }
}
