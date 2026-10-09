package bot.mac.mobile

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.coroutines.launch

object MacBotNotifications {
    const val CHANNEL_NEEDS_YOU = "needs-you"
    const val CHANNEL_COMPLETED = "completed"
    const val CHANNEL_MESSAGES = "messages"
    private const val ACTION_APPROVAL = "bot.mac.mobile.APPROVAL_ACTION"
    private const val LEDGER_PREFS = "macbot_notification_ledger"

    fun ensureChannels(context: Context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
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

    fun postNeedsYou(
        context: Context,
        hostId: String,
        approvalId: String,
        title: String,
        text: String,
        eventSeq: Long? = null,
    ) {
        if (!canNotify(context) || !NotificationLedger.accept(context, "$hostId:approval:$approvalId", eventSeq)) return
        ensureChannels(context)
        val builder = NotificationCompat.Builder(context, CHANNEL_NEEDS_YOU)
            .setSmallIcon(android.R.drawable.ic_dialog_alert)
            .setContentTitle(title)
            .setContentText(text)
            .setStyle(NotificationCompat.BigTextStyle().bigText(text))
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setAutoCancel(true)
            .setContentIntent(activityIntent(context, "approval", approvalId, hostId))
            .addAction(action(context, hostId, approvalId, "allow_once", R.string.approval_allow_once))
            .addAction(action(context, hostId, approvalId, "always_allow", R.string.approval_always_allow))
            .addAction(action(context, hostId, approvalId, "deny", R.string.approval_reject))
        notify(context, (hostId + approvalId).hashCode(), builder)
    }

    fun postCompleted(
        context: Context,
        hostId: String,
        id: String,
        title: String,
        text: String,
        eventSeq: Long? = null,
        projectId: String = id,
    ) {
        if (!canNotify(context) || !NotificationLedger.accept(context, "$hostId:completed:$id", eventSeq)) return
        ensureChannels(context)
        val builder = NotificationCompat.Builder(context, CHANNEL_COMPLETED)
            .setSmallIcon(android.R.drawable.stat_sys_download_done)
            .setContentTitle(title)
            .setContentText(text)
            .setStyle(NotificationCompat.BigTextStyle().bigText(text))
            .setAutoCancel(true)
            .setContentIntent(activityIntent(context, "review", projectId, hostId))
            .addAction(reviewAction(context, hostId, projectId))
        notify(context, (hostId + id).hashCode(), builder)
    }

    fun postMessage(
        context: Context,
        hostId: String,
        id: String,
        title: String,
        text: String,
        chatId: String? = null,
        muted: Boolean = false,
        botNotifications: Boolean = true,
        eventSeq: Long? = null,
    ) {
        if (muted || !botNotifications || !canNotify(context) ||
            !NotificationLedger.accept(context, "$hostId:message:$id", eventSeq)
        ) return
        ensureChannels(context)
        val builder = NotificationCompat.Builder(context, CHANNEL_MESSAGES)
            .setSmallIcon(android.R.drawable.sym_action_chat)
            .setContentTitle(title)
            .setContentText(text)
            .setStyle(NotificationCompat.BigTextStyle().bigText(text))
            .setAutoCancel(true)
        chatId?.let { builder.setContentIntent(activityIntent(context, "chat", it, hostId)) }
        notify(context, (hostId + id).hashCode(), builder)
    }

    /** Called by the event collector for needs-you events other than approvals. */
    fun postNeedsYouEvent(
        context: Context,
        hostId: String,
        id: String,
        title: String,
        text: String,
        deepLinkKind: String,
        deepLinkId: String,
        eventSeq: Long? = null,
    ) {
        if (!canNotify(context) || !NotificationLedger.accept(context, "$hostId:needs:$id", eventSeq)) return
        ensureChannels(context)
        val builder = NotificationCompat.Builder(context, CHANNEL_NEEDS_YOU)
            .setSmallIcon(android.R.drawable.ic_dialog_alert)
            .setContentTitle(title)
            .setContentText(text)
            .setStyle(NotificationCompat.BigTextStyle().bigText(text))
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setAutoCancel(true)
            .setContentIntent(activityIntent(context, deepLinkKind, deepLinkId, hostId))
        notify(context, (hostId + id).hashCode(), builder)
    }

    private fun canNotify(context: Context): Boolean =
        Build.VERSION.SDK_INT < 33 || NotificationManagerCompat.from(context).areNotificationsEnabled()

    private fun notify(context: Context, id: Int, builder: NotificationCompat.Builder) {
        NotificationManagerCompat.from(context).notify(id, builder.build())
    }

    private fun action(context: Context, hostId: String, approvalId: String, action: String, label: Int): NotificationCompat.Action {
        val intent = Intent(context, ApprovalActionReceiver::class.java).apply {
            this.action = ACTION_APPROVAL
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
            putExtra("approval_id", id)
            putExtra("host_id", hostId)
            putExtra("approval_action", "confirm_done")
        }
        val pending = PendingIntent.getBroadcast(context, ("review:$hostId:$id").hashCode(), intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        return NotificationCompat.Action.Builder(android.R.drawable.ic_menu_view, context.getString(R.string.review_completion), pending).build()
    }

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

internal object NotificationLedger {
    private const val LAST_SEQ_PREFIX = "_last_persistent_seq:"

    fun seed(context: Context, hostId: String, seq: Long) {
        val prefs = context.getSharedPreferences(LEDGER_PREFS, Context.MODE_PRIVATE)
        val key = LAST_SEQ_PREFIX + hostId
        val existing = prefs.getLong(key, 0L)
        if (seq > existing) prefs.edit().putLong(key, seq).commit()
    }

    fun acceptSeq(context: Context, hostId: String, seq: Long): Boolean {
        val prefs = context.getSharedPreferences(LEDGER_PREFS, Context.MODE_PRIVATE)
        val key = LAST_SEQ_PREFIX + hostId
        val existing = prefs.getLong(key, 0L)
        if (seq <= existing) return false
        return prefs.edit().putLong(key, seq).commit()
    }

    fun accept(context: Context, key: String, seq: Long?): Boolean {
        val prefs = context.getSharedPreferences(LEDGER_PREFS, Context.MODE_PRIVATE)
        val marker = seq?.toString() ?: "seen"
        if (prefs.getString(key, null) == marker) return false
        return prefs.edit().putString(key, marker).commit()
    }
}
