package bot.mac.mobile.core.protocol

import kotlinx.serialization.KSerializer
import kotlinx.serialization.Serializable
import kotlinx.serialization.descriptors.SerialDescriptor
import kotlinx.serialization.encoding.Decoder
import kotlinx.serialization.encoding.Encoder
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.longOrNull

/** Base for protocol objects: [raw] remains the complete wire object for forward compatibility. */
interface RawProtocolModel { val raw: JsonObject }

abstract class RawModelSerializer<T : RawProtocolModel>(
    private val factory: (JsonObject) -> T,
) : KSerializer<T> {
    override val descriptor: SerialDescriptor = JsonObject.serializer().descriptor
    override fun serialize(encoder: Encoder, value: T) =
        encoder.encodeSerializableValue(JsonObject.serializer(), value.raw)
    override fun deserialize(decoder: Decoder): T =
        factory(decoder.decodeSerializableValue(JsonObject.serializer()))
}

private fun JsonObject.stringOrNull(key: String): String? =
    (this[key] as? JsonPrimitive)?.contentOrNull
private fun JsonObject.longOrNull(key: String): Long? =
    (this[key] as? JsonPrimitive)?.longOrNull
private fun JsonObject.boolOrNull(key: String): Boolean? =
    (this[key] as? JsonPrimitive)?.booleanOrNull
private fun JsonObject.objectOrNull(key: String): JsonObject? = this[key] as? JsonObject
private fun JsonObject.arrayOrEmpty(key: String): JsonArray = this[key] as? JsonArray ?: JsonArray(emptyList())
private fun JsonObject.modelObjects(key: String): List<JsonObject> = arrayOrEmpty(key).mapNotNull { it as? JsonObject }

@Serializable(with = HelloSerializer::class)
data class Hello(override val raw: JsonObject) : RawProtocolModel {
    val protocol: Long get() = raw.longOrNull("protocol") ?: 0L
    val serverVersion: String get() = raw.str("server_version")
    val nodeId: String get() = raw.str("node_id")
    val hostName: String get() = raw.str("host_name")
    val serverTime: String get() = raw.str("server_time")
    val lastSeq: Long get() = raw.long("last_seq")
    val timezone: String get() = raw.str("timezone")
    val currency: String get() = raw.str("currency")
    val features: List<String> get() = raw.arrayOrEmpty("features").mapNotNull { (it as? JsonPrimitive)?.contentOrNull }
}
object HelloSerializer : RawModelSerializer<Hello>(::Hello)

@Serializable(with = SenderSerializer::class)
data class Sender(override val raw: JsonObject) : RawProtocolModel {
    val kind: String get() = raw.str("kind")
    val botId: String? get() = raw.stringOrNull("bot_id")
}
object SenderSerializer : RawModelSerializer<Sender>(::Sender)

@Serializable(with = UsageTotalsSerializer::class)
data class UsageTotals(override val raw: JsonObject) : RawProtocolModel {
    val inputTokens: Long get() = raw.long("input_tokens")
    val outputTokens: Long get() = raw.long("output_tokens")
    val cacheReadTokens: Long get() = raw.long("cache_read_tokens")
    val cacheWriteTokens: Long get() = raw.long("cache_write_tokens")
    val requests: Long get() = raw.long("requests")
    val cost: Double? get() = raw["cost"]?.jsonPrimitive?.doubleOrNull
}
object UsageTotalsSerializer : RawModelSerializer<UsageTotals>(::UsageTotals)

@Serializable(with = FileRefSerializer::class)
data class FileRef(override val raw: JsonObject) : RawProtocolModel {
    val root: String get() = raw.str("root")
    val rootId: String get() = raw.str("root_id")
    val path: String get() = raw.str("path")
    val name: String get() = raw.str("name")
    val size: Long get() = raw.long("size")
    val mime: String get() = raw.str("mime")
}
object FileRefSerializer : RawModelSerializer<FileRef>(::FileRef)

@Serializable(with = ArtifactRefSerializer::class)
data class ArtifactRef(override val raw: JsonObject) : RawProtocolModel {
    val artifactId: String get() = raw.str("artifact_id")
    val title: String get() = raw.str("title")
    val pathOrUrl: String get() = raw.str("path_or_url")
}
object ArtifactRefSerializer : RawModelSerializer<ArtifactRef>(::ArtifactRef)

@Serializable(with = AvatarSerializer::class)
data class Avatar(override val raw: JsonObject) : RawProtocolModel {
    val kind: String get() = raw.str("kind")
    val color: Long? get() = raw.longOrNull("color")
    val emoji: String? get() = raw.stringOrNull("emoji")
    val file: FileRef? get() = raw.objectOrNull("file")?.let(::FileRef)
}
object AvatarSerializer : RawModelSerializer<Avatar>(::Avatar)

@Serializable(with = ToolTogglesSerializer::class)
data class ToolToggles(override val raw: JsonObject) : RawProtocolModel {
    val files: Boolean get() = raw.boolOrNull("files") ?: false
    val bash: Boolean get() = raw.boolOrNull("bash") ?: false
    val browser: Boolean get() = raw.boolOrNull("browser") ?: false
    val subagent: Boolean get() = raw.boolOrNull("subagent") ?: false
    val web: Boolean get() = raw.boolOrNull("web") ?: false
    val mcp: Boolean get() = raw.boolOrNull("mcp") ?: false
}
object ToolTogglesSerializer : RawModelSerializer<ToolToggles>(::ToolToggles)

@Serializable(with = BotStatusSerializer::class)
data class BotStatus(override val raw: JsonObject) : RawProtocolModel {
    val summary: String get() = raw.str("summary")
    val active: Long get() = raw.long("active")
    val queued: Long get() = raw.long("queued")
    val waiting: Long get() = raw.long("waiting")
}
object BotStatusSerializer : RawModelSerializer<BotStatus>(::BotStatus)

@Serializable(with = BotSerializer::class)
data class Bot(override val raw: JsonObject) : RawProtocolModel {
    val id: String get() = raw.str("id")
    val name: String get() = raw.str("name")
    val label: String get() = raw.str("label")
    val description: String get() = raw.str("description")
    val avatar: Avatar? get() = raw.objectOrNull("avatar")?.let(::Avatar)
    val isMain: Boolean get() = raw.boolOrNull("is_main") ?: false
    val model: String? get() = raw.stringOrNull("model")
    val maxParallel: Long get() = raw.long("max_parallel")
    val tools: ToolToggles? get() = raw.objectOrNull("tools")?.let(::ToolToggles)
    val browserMode: String get() = raw.str("browser_mode")
    val pinned: Boolean get() = raw.boolOrNull("pinned") ?: false
    val hidden: Boolean get() = raw.boolOrNull("hidden") ?: false
    val notifications: Boolean get() = raw.boolOrNull("notifications") ?: false
    val dmChatId: String get() = raw.str("dm_chat_id")
    val createdAt: String get() = raw.str("created_at")
    val updatedAt: String get() = raw.str("updated_at")
    val status: BotStatus? get() = raw.objectOrNull("status")?.let(::BotStatus)
}
object BotSerializer : RawModelSerializer<Bot>(::Bot)

@Serializable(with = MessagePreviewSerializer::class)
data class MessagePreview(override val raw: JsonObject) : RawProtocolModel {
    val messageId: String get() = raw.str("message_id")
    val sender: Sender? get() = raw.objectOrNull("sender")?.let(::Sender)
    val text: String get() = raw.str("text")
    val createdAt: String get() = raw.str("created_at")
}
object MessagePreviewSerializer : RawModelSerializer<MessagePreview>(::MessagePreview)

@Serializable(with = ChatSerializer::class)
data class Chat(override val raw: JsonObject) : RawProtocolModel {
    val id: String get() = raw.str("id")
    val kind: String get() = raw.str("kind")
    val title: String get() = raw.str("title")
    val botId: String? get() = raw.stringOrNull("bot_id")
    val projectId: String? get() = raw.stringOrNull("project_id")
    val memberBotIds: List<String> get() = raw.arrayOrEmpty("member_bot_ids").mapNotNull { (it as? JsonPrimitive)?.contentOrNull }
    val lastMessage: MessagePreview? get() = raw.objectOrNull("last_message")?.let(::MessagePreview)
    val lastSeq: Long get() = raw.long("last_seq")
    val lastReadSeq: Long get() = raw.long("last_read_seq")
    val unread: Long get() = raw.long("unread")
    val attention: String get() = raw.str("attention")
    val pinned: Boolean get() = raw.boolOrNull("pinned") ?: false
    val muted: Boolean get() = raw.boolOrNull("muted") ?: false
    val updatedAt: String get() = raw.str("updated_at")
}
object ChatSerializer : RawModelSerializer<Chat>(::Chat)

@Serializable(with = MentionSerializer::class)
data class Mention(override val raw: JsonObject) : RawProtocolModel {
    val kind: String get() = raw.str("kind")
    val botId: String? get() = raw.stringOrNull("bot_id")
    val instruction: String? get() = raw.stringOrNull("instruction")
}
object MentionSerializer : RawModelSerializer<Mention>(::Mention)

@Serializable(with = DeliverySerializer::class)
data class Delivery(override val raw: JsonObject) : RawProtocolModel {
    val botId: String get() = raw.str("bot_id")
    val assignmentId: String? get() = raw.stringOrNull("assignment_id")
    val state: String get() = raw.str("state")
    val at: String get() = raw.str("at")
}
object DeliverySerializer : RawModelSerializer<Delivery>(::Delivery)

@Serializable(with = ReactionSerializer::class)
data class Reaction(override val raw: JsonObject) : RawProtocolModel {
    val emoji: String get() = raw.str("emoji")
    val by: List<Sender> get() = raw.modelObjects("by").map(::Sender)
}
object ReactionSerializer : RawModelSerializer<Reaction>(::Reaction)

@Serializable(with = MessageSerializer::class)
data class Message(override val raw: JsonObject) : RawProtocolModel {
    val id: String get() = raw.str("id")
    val chatId: String get() = raw.str("chat_id")
    val seq: Long get() = raw.long("seq")
    val sender: Sender? get() = raw.objectOrNull("sender")?.let(::Sender)
    val createdAt: String get() = raw.str("created_at")
    val editedAt: String? get() = raw.stringOrNull("edited_at")
    val deleted: Boolean get() = raw.boolOrNull("deleted") ?: false
    val replyTo: String? get() = raw.stringOrNull("reply_to")
    val threadCount: Long get() = raw.long("thread_count")
    val mentions: List<Mention> get() = raw.modelObjects("mentions").map(::Mention)
    val blocks: List<Block> get() = raw.objects("blocks").map(Block::decode)
    val fallbackText: String get() = raw.str("fallback_text")
    val intent: String? get() = raw.stringOrNull("intent")
    val assignmentId: String? get() = raw.stringOrNull("assignment_id")
    val streaming: Boolean get() = raw.boolOrNull("streaming") ?: false
    val delivery: List<Delivery> get() = raw.modelObjects("delivery").map(::Delivery)
    val reactions: List<Reaction> get() = raw.modelObjects("reactions").map(::Reaction)
}
object MessageSerializer : RawModelSerializer<Message>(::Message)

@Serializable(with = ProjectMemberSerializer::class)
data class ProjectMember(override val raw: JsonObject) : RawProtocolModel {
    val botId: String get() = raw.str("bot_id")
    val roleNote: String get() = raw.str("role_note")
    val joinedAt: String get() = raw.str("joined_at")
}
object ProjectMemberSerializer : RawModelSerializer<ProjectMember>(::ProjectMember)

@Serializable(with = ProjectSerializer::class)
data class Project(override val raw: JsonObject) : RawProtocolModel {
    val id: String get() = raw.str("id")
    val chatId: String get() = raw.str("chat_id")
    val name: String get() = raw.str("name")
    val slug: String get() = raw.str("slug")
    val goal: String get() = raw.str("goal")
    val flow: List<String> get() = raw.arrayOrEmpty("flow").mapNotNull { (it as? JsonPrimitive)?.contentOrNull }
    val deadline: String? get() = raw.stringOrNull("deadline")
    val homePath: String get() = raw.str("home_path")
    val status: String get() = raw.str("status")
    val leadBotId: String get() = raw.str("lead_bot_id")
    val members: List<ProjectMember> get() = raw.modelObjects("members").map(::ProjectMember)
    val createdBy: Sender? get() = raw.objectOrNull("created_by")?.let(::Sender)
    val createdAt: String get() = raw.str("created_at")
    val updatedAt: String get() = raw.str("updated_at")
    val doneAt: String? get() = raw.stringOrNull("done_at")
}
object ProjectSerializer : RawModelSerializer<Project>(::Project)

@Serializable(with = AnnouncementSerializer::class)
data class Announcement(override val raw: JsonObject) : RawProtocolModel {
    val projectId: String get() = raw.str("project_id")
    val members: List<JsonObject> get() = raw.objects("members")
    val artifacts: List<Artifact> get() = raw.modelObjects("artifacts").map(::Artifact)
    val highlights: List<JsonObject> get() = raw.modelObjects("highlights")
    val updatedAt: String get() = raw.str("updated_at")
}
object AnnouncementSerializer : RawModelSerializer<Announcement>(::Announcement)

@Serializable(with = AssignmentSerializer::class)
data class Assignment(override val raw: JsonObject) : RawProtocolModel {
    val id: String get() = raw.str("id")
    val projectId: String? get() = raw.stringOrNull("project_id")
    val originChatId: String get() = raw.str("origin_chat_id")
    val botId: String get() = raw.str("bot_id")
    val title: String get() = raw.str("title")
    val instruction: String get() = raw.str("instruction")
    val from: Sender? get() = raw.objectOrNull("from")?.let(::Sender)
    val triggerMessageId: String? get() = raw.stringOrNull("trigger_message_id")
    val parentAssignmentId: String? get() = raw.stringOrNull("parent_assignment_id")
    val status: String get() = raw.str("status")
    val queueReason: String? get() = raw.stringOrNull("queue_reason")
    val wait: JsonObject? get() = raw.objectOrNull("wait")
    val createdAt: String get() = raw.str("created_at")
    val startedAt: String? get() = raw.stringOrNull("started_at")
    val finishedAt: String? get() = raw.stringOrNull("finished_at")
    val usage: UsageTotals? get() = raw.objectOrNull("usage")?.let(::UsageTotals)
    val resultMessageId: String? get() = raw.stringOrNull("result_message_id")
    val model: String get() = raw.str("model")
}
object AssignmentSerializer : RawModelSerializer<Assignment>(::Assignment)

@Serializable(with = ArtifactSerializer::class)
data class Artifact(override val raw: JsonObject) : RawProtocolModel {
    val id: String get() = raw.str("id")
    val projectId: String get() = raw.str("project_id")
    val botId: String get() = raw.str("bot_id")
    val assignmentId: String get() = raw.str("assignment_id")
    val title: String get() = raw.str("title")
    val pathOrUrl: String get() = raw.str("path_or_url")
    val kind: String get() = raw.str("kind")
    val createdAt: String get() = raw.str("created_at")
    val updatedAt: String get() = raw.str("updated_at")
}
object ArtifactSerializer : RawModelSerializer<Artifact>(::Artifact)

@Serializable(with = ApprovalSerializer::class)
data class Approval(override val raw: JsonObject) : RawProtocolModel {
    val id: String get() = raw.str("id")
    val botId: String get() = raw.str("bot_id")
    val assignmentId: String? get() = raw.stringOrNull("assignment_id")
    val chatId: String get() = raw.str("chat_id")
    val tool: String get() = raw.str("tool")
    val risk: String get() = raw.str("risk")
    val summary: String get() = raw.str("summary")
    val detail: String get() = raw.str("detail")
    val state: String get() = raw.str("state")
    val createdAt: String get() = raw.str("created_at")
    val decidedAt: String? get() = raw.stringOrNull("decided_at")
}
object ApprovalSerializer : RawModelSerializer<Approval>(::Approval)

@Serializable(with = QuestionSerializer::class)
data class Question(override val raw: JsonObject) : RawProtocolModel {
    val id: String get() = raw.str("id")
    val botId: String get() = raw.str("bot_id")
    val assignmentId: String get() = raw.str("assignment_id")
    val chatId: String get() = raw.str("chat_id")
    val text: String get() = raw.str("text")
    val options: List<String> get() = raw.arrayOrEmpty("options").mapNotNull { (it as? JsonPrimitive)?.contentOrNull }
    val allowFreeText: Boolean get() = raw.boolOrNull("allow_free_text") ?: false
    val state: String get() = raw.str("state")
    val answer: JsonObject? get() = raw.objectOrNull("answer")
}
object QuestionSerializer : RawModelSerializer<Question>(::Question)

@Serializable(with = SkillSerializer::class)
data class Skill(override val raw: JsonObject) : RawProtocolModel {
    val name: String get() = raw.str("name")
    val description: String get() = raw.str("description")
    val source: String get() = raw.str("source")
    val path: String get() = raw.str("path")
    val files: List<String> get() = raw.arrayOrEmpty("files").mapNotNull { (it as? JsonPrimitive)?.contentOrNull }
    val enabled: Boolean get() = raw.boolOrNull("enabled") ?: false
    val disabledBotIds: List<String> get() = raw.arrayOrEmpty("disabled_bot_ids").mapNotNull { (it as? JsonPrimitive)?.contentOrNull }
    val invocations7d: JsonObject? get() = raw.objectOrNull("invocations_7d")
    val updatedAt: String get() = raw.str("updated_at")
    val content: String? get() = raw.stringOrNull("content")
}
object SkillSerializer : RawModelSerializer<Skill>(::Skill)

@Serializable(with = RoutineRunSerializer::class)
data class RoutineRun(override val raw: JsonObject) : RawProtocolModel {
    val id: String get() = raw.str("id")
    val routineId: String get() = raw.str("routine_id")
    val assignmentId: String? get() = raw.stringOrNull("assignment_id")
    val trigger: String get() = raw.str("trigger")
    val status: String get() = raw.str("status")
    val startedAt: String get() = raw.str("started_at")
    val finishedAt: String? get() = raw.stringOrNull("finished_at")
    val error: String? get() = raw.stringOrNull("error")
}
object RoutineRunSerializer : RawModelSerializer<RoutineRun>(::RoutineRun)

@Serializable(with = RoutineSerializer::class)
data class Routine(override val raw: JsonObject) : RawProtocolModel {
    val id: String get() = raw.str("id")
    val botId: String get() = raw.str("bot_id")
    val projectId: String? get() = raw.stringOrNull("project_id")
    val name: String get() = raw.str("name")
    val instructions: String get() = raw.str("instructions")
    val schedules: List<JsonObject> get() = raw.objects("schedules")
    val timezone: String get() = raw.str("timezone")
    val enabled: Boolean get() = raw.boolOrNull("enabled") ?: false
    val nextRunAt: String? get() = raw.stringOrNull("next_run_at")
    val lastRun: RoutineRun? get() = raw.objectOrNull("last_run")?.let(::RoutineRun)
    val createdAt: String get() = raw.str("created_at")
    val updatedAt: String get() = raw.str("updated_at")
}
object RoutineSerializer : RawModelSerializer<Routine>(::Routine)

@Serializable(with = ProviderSerializer::class)
data class Provider(override val raw: JsonObject) : RawProtocolModel {
    val id: String get() = raw.str("id")
    val name: String get() = raw.str("name")
    val apiKind: String get() = raw.str("api_kind")
    val baseUrl: String get() = raw.str("base_url")
    val hasKey: Boolean get() = raw.boolOrNull("has_key") ?: false
    val headers: JsonObject get() = raw.objectOrNull("headers") ?: JsonObject(emptyMap())
    val createdAt: String get() = raw.str("created_at")
    val updatedAt: String get() = raw.str("updated_at")
}
object ProviderSerializer : RawModelSerializer<Provider>(::Provider)

@Serializable(with = ModelSerializer::class)
data class Model(override val raw: JsonObject) : RawProtocolModel {
    val ref: String get() = raw.str("ref")
    val providerId: String get() = raw.str("provider_id")
    val modelId: String get() = raw.str("model_id")
    val displayName: String get() = raw.str("display_name")
    val contextWindow: Long get() = raw.long("context_window")
    val maxOutput: Long get() = raw.long("max_output")
    val caps: JsonObject? get() = raw.objectOrNull("caps")
    val price: JsonObject? get() = raw.objectOrNull("price")
    val enabled: Boolean get() = raw.boolOrNull("enabled") ?: false
}
object ModelSerializer : RawModelSerializer<Model>(::Model)

@Serializable(with = SettingsSerializer::class)
data class Settings(override val raw: JsonObject) : RawProtocolModel {
    val hostName: String get() = raw.str("host_name")
    val timezone: String get() = raw.str("timezone")
    val currency: String get() = raw.str("currency")
    val concurrency: JsonObject? get() = raw.objectOrNull("concurrency")
    val models: JsonObject? get() = raw.objectOrNull("models")
    val mainBot: JsonObject? get() = raw.objectOrNull("main_bot")
    val approvals: JsonObject? get() = raw.objectOrNull("approvals")
    val browser: JsonObject? get() = raw.objectOrNull("browser")
    val skills: JsonObject? get() = raw.objectOrNull("skills")
    val trace: JsonObject? get() = raw.objectOrNull("trace")
    val webSearch: JsonObject? get() = raw.objectOrNull("web_search")
    val push: JsonObject? get() = raw.objectOrNull("push")
}
object SettingsSerializer : RawModelSerializer<Settings>(::Settings)

@Serializable(with = DeviceSerializer::class)
data class Device(override val raw: JsonObject) : RawProtocolModel {
    val id: String get() = raw.str("id")
    val platform: String get() = raw.str("platform")
    val appVersion: String get() = raw.str("app_version")
    val deviceName: String get() = raw.str("device_name")
    val pushToken: String? get() = raw.stringOrNull("push_token")
    val lastSeenAt: String get() = raw.str("last_seen_at")
}
object DeviceSerializer : RawModelSerializer<Device>(::Device)

@Serializable(with = TraceItemSerializer::class)
data class TraceItem(override val raw: JsonObject) : RawProtocolModel {
    val assignmentId: String? get() = raw.stringOrNull("assignment_id")
    val chatId: String get() = raw.str("chat_id")
    val runId: String get() = raw.str("run_id")
    val aseq: Long get() = raw.long("aseq")
    val at: String get() = raw.str("at")
    val type: String get() = raw.str("type")
    val data: JsonObject get() = raw.objectOrNull("data") ?: JsonObject(emptyMap())
}
object TraceItemSerializer : RawModelSerializer<TraceItem>(::TraceItem)

@Serializable(with = PendingItemsSerializer::class)
data class PendingItems(override val raw: JsonObject) : RawProtocolModel {
    val approvals: List<Approval> get() = raw.modelObjects("approvals").map(::Approval)
    val questions: List<Question> get() = raw.modelObjects("questions").map(::Question)
    val reviews: List<String> get() = raw.arrayOrEmpty("reviews").mapNotNull { (it as? JsonPrimitive)?.contentOrNull }
}
object PendingItemsSerializer : RawModelSerializer<PendingItems>(::PendingItems)
