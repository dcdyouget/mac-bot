package bot.mac.mobile.feature.memory

import kotlinx.datetime.Instant
import kotlinx.datetime.TimeZone
import kotlinx.datetime.toLocalDateTime

internal fun formatMemoryTimestamp(value: String): String = runCatching {
    val local = Instant.parse(value).toLocalDateTime(TimeZone.currentSystemDefault())
    "${(local.month.ordinal + 1).toString().padStart(2, '0')}/${local.day.toString().padStart(2, '0')} ${local.hour.toString().padStart(2, '0')}:${local.minute.toString().padStart(2, '0')}"
}.getOrElse { value }
