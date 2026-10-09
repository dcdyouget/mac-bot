package bot.mac.mobile

import android.Manifest
import android.annotation.SuppressLint
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import bot.mac.mobile.core.state.AppRuntime
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.coroutines.launch
import kotlinx.serialization.encodeToString

private const val NOTIFICATION_LEDGER_PREFS = "macbot_notification_ledger"

object MacBotNotifications {
    const val CHANNEL_NEEDS_YOU = "needs-you"
    const val CHANNEL_COMPLETED = "completed"
    const val CHANNEL_MESSAGES = "messages"
    internal const val DELIVERY_KEY = "macbot.delivery.key"
    internal const val DELIVERY_MARKER = "macbot.delivery.marker"
    private const val ACTION_APPROVAL = "bot.mac.mobile.APPROVAL_ACTION"
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val wake = kotlinx.coroutines.channels.Channel<Unit>(kotlinx.coroutines.channels.Channel.CONFLATED)
    private val workerLock = Any()
    private var coordinator: NotificationDeliveryCoordinator? = null
    private var worker: kotlinx.coroutines.Job? = null

    fun ensureChannels(context: Context) {
        val manager = context.getSystemService(NotificationManager::class.java)
        manager.createNotificationChannels(
            listOf(
                NotificationChannel(CHANNEL_NEEDS_YOU, context.getString(R.string.channel_needs_you), NotificationManager.IMPORTANCE_HIGH).apply {
                    description = context.getString(R.string.channel_needs_you_description)
                },
                NotificationChannel(CHANNEL_COMPLETED, context.getString(R.string.channel_completed), NotificationManager.IMPORTANCE_DEFAULT),
                NotificationChannel(CHANNEL_MESSAGES, context.getString(R.string.channel_messages), NotificationManager.IMPORTANCE_DEFAULT),
            ),
        )
    }

    suspend fun postNeedsYou(context: Context, hostId: String, approvalId: String, title: String, text: String, eventSeq: Long? = null) =
        enqueue(context, PendingNotification(NotificationKind.APPROVAL, hostId, approvalId, title, text, eventSeq))

    suspend fun postCompleted(
        context: Context, hostId: String, id: String, title: String, text: String,
        eventSeq: Long? = null, projectId: String? = null, chatId: String? = null,
    ) {
        if (projectId.isNullOrBlank() && chatId.isNullOrBlank()) return
        enqueue(context, PendingNotification(NotificationKind.COMPLETED, hostId, id, title, text, eventSeq, projectId, chatId))
    }

    suspend fun postMessage(
        context: Context, hostId: String, id: String, title: String, text: String,
        chatId: String? = null, muted: Boolean = false, botNotifications: Boolean = true, eventSeq: Long? = null,
    ) {
        if (muted || !botNotifications) return
        enqueue(context, PendingNotification(NotificationKind.MESSAGE, hostId, id, title, text, eventSeq, chatId = chatId))
    }

    suspend fun postNeedsYouEvent(
        context: Context, hostId: String, id: String, title: String, text: String,
        deepLinkKind: String, deepLinkId: String, eventSeq: Long? = null,
    ) = enqueue(context, PendingNotification(
        NotificationKind.NEEDS_YOU, hostId, id, title, text, eventSeq,
        deepLinkKind = deepLinkKind, deepLinkId = deepLinkId,
    ))

    private suspend fun enqueue(context: Context, request: PendingNotification) {
        if (!AppRuntime.repository.notifications.value) return
        delivery(context).enqueue(request)
        resumePending(context)
    }

    private fun delivery(context: Context): NotificationDeliveryCoordinator = synchronized(workerLock) {
        coordinator ?: NotificationDeliveryCoordinator(
            AndroidNotificationDeliveryStore(context.applicationContext),
            AndroidNotificationDeliveryBackend(context.applicationContext),
        ).also { coordinator = it }
    }

    /** Called after repository settings initialize; never runs against a stale toggle. */
    fun resumePending(context: Context) {
        val delivery = delivery(context)
        synchronized(workerLock) {
            if (worker?.isActive != true) worker = scope.launch {
                while (kotlin.coroutines.coroutineContext[kotlinx.coroutines.Job]?.isActive == true) {
                    wake.receive()
                    var retryMillis = 1_000L
                    do {
                        val remaining = try {
                            delivery.deliverPending()
                        } catch (failure: Throwable) {
                            if (failure is kotlinx.coroutines.CancellationException) throw failure
                            // Log metadata only. The durable queue retains rejected deliveries.
                            android.util.Log.w("MacBotNotifications", "Notification delivery deferred: ${failure.javaClass.simpleName}")
                            true
                        }
                        if (remaining) {
                            kotlinx.coroutines.delay(retryMillis)
                            retryMillis = (retryMillis * 2).coerceAtMost(30_000L)
                        }
                    } while (remaining)
                }
            }
        }
        wake.trySend(Unit)
    }

    internal fun notification(context: Context, request: PendingNotification): android.app.Notification {
        val channel = when (request.kind) {
            NotificationKind.APPROVAL, NotificationKind.NEEDS_YOU -> CHANNEL_NEEDS_YOU
            NotificationKind.COMPLETED -> CHANNEL_COMPLETED
            NotificationKind.MESSAGE -> CHANNEL_MESSAGES
        }
        val icon = when (request.kind) {
            NotificationKind.APPROVAL, NotificationKind.NEEDS_YOU -> android.R.drawable.ic_dialog_alert
            NotificationKind.COMPLETED -> android.R.drawable.stat_sys_download_done
            NotificationKind.MESSAGE -> android.R.drawable.sym_action_chat
        }
        val extras = android.os.Bundle().apply {
            putString(DELIVERY_KEY, request.ledgerKey)
            putString(DELIVERY_MARKER, request.marker)
        }
        val builder = NotificationCompat.Builder(context, channel)
            .setSmallIcon(icon)
            .setContentTitle(request.title)
            .setContentText(request.text)
            .setStyle(NotificationCompat.BigTextStyle().bigText(request.text))
            .setAutoCancel(true)
            .setOnlyAlertOnce(request.kind != NotificationKind.MESSAGE)
            .addExtras(extras)
        when (request.kind) {
            NotificationKind.APPROVAL -> {
                builder.setPriority(NotificationCompat.PRIORITY_HIGH)
                    .setContentIntent(activityIntent(context, "approval", request.id, request.hostId))
                    .addAction(action(context, request.hostId, request.id, "allow_once", R.string.approval_allow_once))
                    .addAction(action(context, request.hostId, request.id, "always_allow", R.string.approval_always_allow))
                    .addAction(action(context, request.hostId, request.id, "deny", R.string.approval_reject))
            }
            NotificationKind.NEEDS_YOU -> builder.setPriority(NotificationCompat.PRIORITY_HIGH)
                .setContentIntent(activityIntent(context, requireNotNull(request.deepLinkKind), requireNotNull(request.deepLinkId), request.hostId))
            NotificationKind.COMPLETED -> {
                val project = request.projectId?.takeIf { it.isNotBlank() }
                val target = project ?: requireNotNull(request.chatId)
                builder.setContentIntent(activityIntent(context, if (project != null) "review" else "chat", target, request.hostId))
                project?.let { builder.addAction(reviewAction(context, request.hostId, it)) }
            }
            NotificationKind.MESSAGE -> request.chatId?.let {
                builder.setContentIntent(activityIntent(context, "chat", it, request.hostId))
            }
        }
        return builder.build()
    }

    internal fun canNotify(context: Context): Boolean = NotificationManagerCompat.from(context).areNotificationsEnabled() &&
        (Build.VERSION.SDK_INT < 33 || ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED)

    private fun action(context: Context, hostId: String, approvalId: String, action: String, label: Int): NotificationCompat.Action {
        val intent = Intent(context, ApprovalActionReceiver::class.java).apply {
            this.action = ACTION_APPROVAL
            data = actionUri(hostId, approvalId, action)
            putExtra("approval_id", approvalId)
            putExtra("host_id", hostId)
            putExtra("approval_action", action)
        }
        val pending = PendingIntent.getBroadcast(context, (hostId + approvalId + action).hashCode(), intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        return NotificationCompat.Action.Builder(android.R.drawable.ic_menu_send, context.getString(label), pending).build()
    }

    private fun reviewAction(context: Context, hostId: String, id: String): NotificationCompat.Action {
        val intent = Intent(context, ApprovalActionReceiver::class.java).apply {
            action = ACTION_APPROVAL
            data = actionUri(hostId, id, "confirm_done")
            putExtra("approval_id", id)
            putExtra("host_id", hostId)
            putExtra("approval_action", "confirm_done")
        }
        val pending = PendingIntent.getBroadcast(context, ("review:$hostId:$id").hashCode(), intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        return NotificationCompat.Action.Builder(android.R.drawable.ic_menu_view, context.getString(R.string.review_completion), pending).build()
    }

    private fun actionUri(hostId: String, id: String, action: String): Uri = Uri.Builder()
        .scheme("macbot").authority("notification-action").appendPath(hostId).appendPath(id).appendPath(action).build()

    private fun activityIntent(context: Context, kind: String, id: String, hostId: String): PendingIntent {
        val intent = Intent(context, MainActivity::class.java).apply {
            data = Uri.Builder().scheme("macbot").authority(kind).appendPath(id).apply {
                if (hostId.isNotBlank()) appendQueryParameter("host_id", hostId)
            }.build()
            putExtra("deep_link_kind", kind)
            putExtra("deep_link_id", id)
            putExtra("host_id", hostId)
            flags = Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP
        }
        return PendingIntent.getActivity(context, (kind + hostId + id).hashCode(), intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
    }
}

internal fun buildNotification(context: Context, request: PendingNotification): android.app.Notification =
    MacBotNotifications.notification(context, request)

internal class AndroidNotificationDeliveryStore(context: Context) : NotificationDeliveryStore {
    private val prefs = context.getSharedPreferences(NOTIFICATION_LEDGER_PREFS, Context.MODE_PRIVATE)
    private val json = kotlinx.serialization.json.Json { ignoreUnknownKeys = true }
    override fun pending(): List<PendingNotification> = prefs.getString("_pending_deliveries", null)?.let {
        json.decodeFromString<List<PendingNotification>>(it)
    }.orEmpty()
    override fun savePending(value: List<PendingNotification>) {
        check(prefs.edit().putString("_pending_deliveries", json.encodeToString(value)).commit()) { "Unable to persist pending notifications" }
    }
    override fun marker(key: String): String? = prefs.getString(key, null)
    override fun markShown(key: String, marker: String) {
        check(prefs.edit().putString(key, marker).commit()) { "Unable to persist notification delivery" }
    }
}

internal class AndroidNotificationDeliveryBackend(
    private val context: Context,
    private val notificationsEnabled: () -> Boolean = { AppRuntime.repository.notifications.value },
) : NotificationDeliveryBackend {
    private val manager = context.getSystemService(NotificationManager::class.java)
    private var prepared: Pair<PendingNotification, android.app.Notification>? = null
    override fun allowed(): Boolean = MacBotNotifications.canNotify(context) && notificationsEnabled()
    override fun allowed(request: PendingNotification): Boolean {
        val channel = when (request.kind) {
            NotificationKind.APPROVAL, NotificationKind.NEEDS_YOU -> MacBotNotifications.CHANNEL_NEEDS_YOU
            NotificationKind.COMPLETED -> MacBotNotifications.CHANNEL_COMPLETED
            NotificationKind.MESSAGE -> MacBotNotifications.CHANNEL_MESSAGES
        }
        return allowed() && manager.getNotificationChannel(channel)?.importance != NotificationManager.IMPORTANCE_NONE
    }
    override fun active(): List<RetainedNotification> = manager.activeNotifications.map { record ->
        val notification = record.notification
        val priority = when (notification.channelId) {
            MacBotNotifications.CHANNEL_NEEDS_YOU -> 2
            MacBotNotifications.CHANNEL_COMPLETED -> if (notification.actions.isNullOrEmpty()) 1 else 2
            else -> 0
        }
        RetainedNotification(
            NotificationIdentity(record.tag, record.id), record.postTime, priority,
            `protected` = record.id == 100 || notification.flags and (android.app.Notification.FLAG_FOREGROUND_SERVICE or android.app.Notification.FLAG_GROUP_SUMMARY) != 0,
        )
    }
    override fun cancel(identity: NotificationIdentity) { manager.cancel(identity.tag, identity.id) }
    override fun prepare(request: PendingNotification) {
        MacBotNotifications.ensureChannels(context)
        prepared = request to buildNotification(context, request)
    }
    override fun post(request: PendingNotification) {
        if (prepared?.first != request) prepare(request)
        manager.notify(request.identity.tag, request.identity.id, requireNotNull(prepared).second)
        prepared = null
    }
    override fun shown(request: PendingNotification): Boolean = manager.activeNotifications.any { record ->
        record.id == request.identity.id && record.tag == request.identity.tag &&
            record.notification.extras.getString(MacBotNotifications.DELIVERY_KEY) == request.ledgerKey &&
            record.notification.extras.getString(MacBotNotifications.DELIVERY_MARKER) == request.marker
    }
}

object ApprovalActionBridge {
    @Volatile var handler: (suspend (hostId: String, approvalId: String, action: String) -> Unit)? = null
    private val ready = CompletableDeferred<Unit>()
    @Volatile var reviewHandler: (suspend (hostId: String, projectId: String) -> Unit)? = null
    private val reviewReady = CompletableDeferred<Unit>()

    fun install(value: suspend (hostId: String, approvalId: String, action: String) -> Unit) {
        handler = value
        if (!ready.isCompleted) ready.complete(Unit)
    }

    fun installReview(value: suspend (hostId: String, projectId: String) -> Unit) {
        reviewHandler = value
        if (!reviewReady.isCompleted) reviewReady.complete(Unit)
    }

    suspend fun awaitReady(context: Context, review: Boolean = false) {
        if (handler == null || (review && reviewHandler == null)) MainConnectionService.start(context)
        withTimeoutOrNull(15_000) { if (review) reviewReady.await() else ready.await() }
    }
}

class ApprovalActionReceiver : BroadcastReceiver() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    override fun onReceive(context: Context, intent: Intent) {
        val id = intent.getStringExtra("approval_id") ?: return
        val hostId = intent.getStringExtra("host_id") ?: return
        val action = intent.getStringExtra("approval_action") ?: return
        val pending = goAsync()
        scope.launch {
            try {
                ApprovalActionBridge.awaitReady(context, review = action == "confirm_done")
                if (action == "confirm_done") {
                    ApprovalActionBridge.reviewHandler?.invoke(hostId, id)
                } else {
                    ApprovalActionBridge.handler?.invoke(hostId, id, action)
                }
            } finally {
                pending.finish()
            }
        }
    }
}

@SuppressLint("ApplySharedPref", "UseKtx")
internal object NotificationLedger {
    private const val LAST_SEQ_PREFIX = "_last_persistent_seq:"

    fun seed(context: Context, hostId: String, seq: Long) {
        val prefs = context.getSharedPreferences(NOTIFICATION_LEDGER_PREFS, Context.MODE_PRIVATE)
        val key = LAST_SEQ_PREFIX + hostId
        // Seeding is only a baseline for a brand-new ledger. During service
        // startup, initialize() may already have delivered a persistent event
        // to the collector; never overwrite that event's accepted sequence.
        if (!prefs.contains(key)) prefs.edit().putLong(key, seq).commit()
    }

    fun acceptSeq(context: Context, hostId: String, seq: Long): Boolean {
        val prefs = context.getSharedPreferences(NOTIFICATION_LEDGER_PREFS, Context.MODE_PRIVATE)
        val key = LAST_SEQ_PREFIX + hostId
        val existing = prefs.getLong(key, 0L)
        if (seq <= existing) return false
        return prefs.edit().putLong(key, seq).commit()
    }

    fun isFreshSeq(context: Context, hostId: String, seq: Long): Boolean =
        seq > context.getSharedPreferences(NOTIFICATION_LEDGER_PREFS, Context.MODE_PRIVATE)
            .getLong(LAST_SEQ_PREFIX + hostId, 0L)

    fun accept(context: Context, key: String, seq: Long?): Boolean {
        val prefs = context.getSharedPreferences(NOTIFICATION_LEDGER_PREFS, Context.MODE_PRIVATE)
        val marker = seq?.toString() ?: "seen"
        if (prefs.getString(key, null) == marker) return false
        return prefs.edit().putString(key, marker).commit()
    }
}
