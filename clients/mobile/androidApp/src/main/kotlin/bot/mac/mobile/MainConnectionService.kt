package bot.mac.mobile

import android.app.Notification
import android.app.Service
import android.content.Context
import android.content.Intent
import android.os.IBinder
import androidx.core.content.ContextCompat
import bot.mac.mobile.core.platform.initializePlatform
import bot.mac.mobile.core.network.MainEvent
import bot.mac.mobile.core.protocol.obj
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.protocol.boolean
import bot.mac.mobile.core.protocol.arr
import bot.mac.mobile.core.state.AppRuntime
import bot.mac.mobile.core.state.MobileState
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.isActive
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

class MainConnectionService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    override fun onCreate() {
        super.onCreate()
        initializePlatform(applicationContext)
        MacBotNotifications.ensureChannels(this)
        startForeground(NOTIFICATION_ID, foregroundNotification(this))
        scope.launch {
            while (isActive) {
                try {
                    // Subscribe before starting Hosts. SharedFlow has no replay, so
                    // subscribing after initialize can lose events during the first
                    // handshake/replay window.
                    val notifications = launch(start = CoroutineStart.UNDISPATCHED) {
                        AppRuntime.repository.hostEvents.collect { hostEvent ->
                            if (!AppRuntime.repository.notifications.value) return@collect
                            try {
                                NotificationEventRouter.handle(this@MainConnectionService, hostEvent.hostId, hostEvent.event, hostEvent.state)
                            } catch (failure: Throwable) {
                                if (failure is CancellationException) throw failure
                                // A malformed notification must not stop delivery for other Hosts.
                            }
                        }
                    }
                    try {
                        ApprovalActionBridge.install { hostId, approvalId, action ->
                            val decision = when (action) {
                                "allow_once" -> "allow_once"
                                "always_allow" -> "always_allow"
                                else -> "deny"
                            }
                            AppRuntime.repository.callOnHost(hostId, "approval.decide", buildJsonObject {
                                put("approval_id", approvalId)
                                put("decision", decision)
                            })
                        }
                        ApprovalActionBridge.installReview { hostId, projectId ->
                            AppRuntime.repository.callOnHost(hostId, "project.confirm_done", buildJsonObject {
                                put("project_id", projectId)
                            })
                        }
                        AppRuntime.repository.initialize()
                        MacBotNotifications.resumePending(this@MainConnectionService)
                        AppRuntime.repository.background()
                    } finally {
                        notifications.cancel()
                    }
                } catch (cancelled: CancellationException) {
                    throw cancelled
                } catch (_: Throwable) {
                    delay(1_000)
                }
            }
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int = START_STICKY
    override fun onBind(intent: Intent?): IBinder? = null
    override fun onDestroy() { scope.cancel(); super.onDestroy() }

    companion object {
        private const val NOTIFICATION_ID = 100
        fun start(context: Context) {
            ContextCompat.startForegroundService(context, Intent(context, MainConnectionService::class.java))
        }

        private fun foregroundNotification(context: Context): Notification =
            MacBotNotifications.foregroundServiceNotification(context)
    }
}

private object NotificationEventRouter {
    suspend fun handle(context: Context, hostId: String, event: MainEvent, snapshot: MobileState) {
        if (event is MainEvent.Hello) {
            // Hello precedes replay. This is the cached cursor, before any newly
            // received persistent events can advance the repository snapshot.
            NotificationLedger.seed(context, hostId, snapshot.lastSeq)
            return
        }
        val data = when (event) {
            is MainEvent.Persistent -> event.data
            is MainEvent.Ephemeral -> event.data
            is MainEvent.Hello -> return
        }
        val eventName = when (event) {
            is MainEvent.Persistent -> event.event
            is MainEvent.Ephemeral -> event.event
            is MainEvent.Hello -> return
        }
        val seq = (event as? MainEvent.Persistent)?.seq
        if (seq != null && !NotificationLedger.isFreshSeq(context, hostId, seq)) return
        when {
            eventName == "approval.requested" -> {
                val approval = data.obj("approval").takeUnless { it.isEmpty() } ?: data
                val id = approval.str("id")
                if (id.isNotBlank()) MacBotNotifications.postNeedsYou(
                    context, hostId, id, context.getString(R.string.notification_approval_title),
                    approval.str("detail").ifBlank { approval.str("summary") }, seq,
                )
            }
            eventName == "question.asked" -> {
                val question = data.obj("question").takeUnless { it.isEmpty() } ?: data
                val id = question.str("id").ifBlank { question.str("question_id") }
                if (id.isNotBlank()) MacBotNotifications.postNeedsYouEvent(
                    context, hostId, id, context.getString(R.string.notification_question_title),
                    question.str("text").ifBlank { question.str("prompt") },
                    "chat", question.str("chat_id"), seq, foregroundAttentionHandled = true,
                )
            }
            eventName == "takeover.requested" -> {
                val botId = data.str("bot_id")
                val id = data.str("id").ifBlank { data.str("request_id") }.ifBlank { botId }
                if (id.isNotBlank() && botId.isNotBlank()) MacBotNotifications.postNeedsYouEvent(
                    context, hostId, id, context.getString(R.string.notification_takeover_title),
                    data.str("reason").ifBlank { context.getString(R.string.notification_takeover_default_text) },
                    "computer", botId, seq, foregroundAttentionHandled = true,
                )
            }
            eventName == "message.created" || eventName == "message.updated" -> {
                val message = data.obj("message").takeUnless { it.isEmpty() } ?: data
                val id = message.str("id")
                val chatId = message.str("chat_id")
                if (id.isBlank() || chatId.isBlank()) return
                // A streaming placeholder is completed by a later persistent
                // message.updated event. Notify only once the final message is
                // available, otherwise the empty placeholder and final body
                // produce two notifications.
                if (!shouldNotifyMessage(message)) return
                val chat = snapshot.chats.firstOrNull { it.str("id") == chatId }
                val sender = message.obj("sender")
                val route = classifyMessageNotification(
                    message = message,
                    chatKind = chat?.str("kind"),
                    notifyAllGroupMessages = AppRuntime.repository.notifyGroupMessages.value,
                )
                val takeover = message.arr("blocks")
                    .asSequence()
                    .mapNotNull { it as? JsonObject }
                    .firstOrNull { it.str("type") == "takeover_request" && it.str("state") == "pending" }
                val takeoverBotId = takeover?.str("bot_id").orEmpty()
                if (route == MessageNotificationRoute.NEEDS_YOU && takeoverBotId.isNotBlank()) {
                    MacBotNotifications.postNeedsYouEvent(
                        context,
                        hostId,
                        id,
                        context.getString(R.string.notification_takeover_title),
                        takeover?.str("reason").orEmpty().ifBlank { context.getString(R.string.notification_takeover_default_text) },
                        "computer",
                        takeoverBotId,
                        null,
                    )
                } else if (route == MessageNotificationRoute.NEEDS_YOU) {
                    MacBotNotifications.postNeedsYouEvent(
                        context, hostId, id, context.getString(R.string.notification_message_needs_you_title),
                        message.str("fallback_text"), "chat", chatId, null,
                        category = if (mentionsUser(message)) "messages" else "needs_you",
                    )
                } else if (route == MessageNotificationRoute.PRIVATE_MESSAGE || route == MessageNotificationRoute.GROUP_MESSAGE) {
                    val botId = sender.str("bot_id")
                    val notifications = snapshot.bots
                        .firstOrNull { it.str("id") == botId }?.boolean("notifications") ?: true
                    MacBotNotifications.postMessage(
                        context, hostId, id, message.str("sender_name").ifBlank { context.getString(R.string.app_name) },
                        message.str("fallback_text"),
                        chatId, chat?.boolean("muted") ?: false, notifications, null,
                        if (route == MessageNotificationRoute.GROUP_MESSAGE) "group_messages" else "messages",
                    )
                }
            }
            eventName == "assignment.updated" -> {
                val assignment = data.obj("assignment").takeUnless { it.isEmpty() } ?: data
                if (assignment.str("status") == "done") {
                    val id = assignment.str("id").ifBlank { assignment.str("assignment_id") }
                    if (id.isNotBlank()) {
                        val title = assignment.str("title").ifBlank { assignment.str("summary") }
                        val projectId = assignment.str("project_id")
                        if (projectId.isNotBlank()) {
                            MacBotNotifications.postCompleted(
                                context, hostId, id, context.getString(R.string.notification_assignment_done_title), title,
                                eventSeq = seq, projectId = projectId,
                            )
                        } else {
                            MacBotNotifications.postCompleted(
                                context, hostId, id, context.getString(R.string.notification_assignment_done_title), title,
                                eventSeq = seq,
                                chatId = assignment.str("origin_chat_id").ifBlank { assignment.str("chat_id") },
                            )
                        }
                    }
                } else if ((assignment.str("status") == "blocked" || assignment.str("status") == "waiting_user") &&
                    assignmentMentionsUser(assignment, snapshot)
                ) {
                    val id = assignment.str("id").ifBlank { assignment.str("assignment_id") }
                    val chatId = assignment.str("origin_chat_id").ifBlank { assignment.str("chat_id") }
                    if (id.isNotBlank()) MacBotNotifications.postNeedsYouEvent(
                        context, hostId, id, context.getString(R.string.notification_assignment_attention_title),
                        assignment.str("title").ifBlank { assignment.str("summary") },
                        "chat", chatId, seq,
                    )
                }
            }
            eventName == "project.updated" -> {
                val project = data.obj("project").takeUnless { it.isEmpty() } ?: data
                if (project.str("status") == "review") {
                    val id = project.str("id").ifBlank { project.str("project_id") }
                    if (id.isNotBlank()) MacBotNotifications.postCompleted(
                        context, hostId, id, context.getString(R.string.notification_project_review_title),
                        project.str("name").ifBlank { project.str("goal") }, eventSeq = seq, projectId = id, category = "review",
                        foregroundAttentionHandled = true,
                    )
                }
            }
        }
        // Delivery payloads are durable before advancing the notification cursor.
        if (seq != null) NotificationLedger.acceptSeq(context, hostId, seq)
    }

    private fun mentionsUser(value: JsonObject): Boolean =
        value.arr("mentions").any { (it as? JsonObject)?.str("kind") == "user" }

    private fun assignmentMentionsUser(assignment: JsonObject, snapshot: MobileState): Boolean {
        if (mentionsUser(assignment)) return true
        val resultMessageId = assignment.str("result_message_id")
        if (resultMessageId.isBlank()) return false
        return snapshot.messages.values.asSequence()
            .flatten()
            .firstOrNull { it.str("id") == resultMessageId }
            ?.let(::mentionsUser) == true
    }

}

internal fun shouldNotifyMessage(message: JsonObject): Boolean =
    !message.boolean("streaming") &&
        message.arr("blocks").none { (it as? JsonObject)?.str("type") == "review_card" }

internal enum class MessageNotificationRoute { IGNORE, NEEDS_YOU, PRIVATE_MESSAGE, GROUP_MESSAGE }

internal fun classifyMessageNotification(
    message: JsonObject,
    chatKind: String?,
    notifyAllGroupMessages: Boolean,
): MessageNotificationRoute {
    if (!shouldNotifyMessage(message)) return MessageNotificationRoute.IGNORE
    val sender = message.obj("sender")
    val senderKind = sender.str("kind")
    // Echoed messages authored by this client must never notify, even if the
    // payload contains a user mention.
    if (senderKind == "user" || senderKind != "bot") return MessageNotificationRoute.IGNORE
    val hasPendingTakeover = message.arr("blocks").any { raw ->
        val block = raw as? JsonObject ?: return@any false
        block.str("type") == "takeover_request" && block.str("state") == "pending"
    }
    if (hasPendingTakeover || message.arr("mentions").any { (it as? JsonObject)?.str("kind") == "user" }) {
        return MessageNotificationRoute.NEEDS_YOU
    }
    return when {
        chatKind in setOf("main", "direct", "bot_dm") -> MessageNotificationRoute.PRIVATE_MESSAGE
        chatKind == "project" && notifyAllGroupMessages -> MessageNotificationRoute.GROUP_MESSAGE
        else -> MessageNotificationRoute.IGNORE
    }
}
