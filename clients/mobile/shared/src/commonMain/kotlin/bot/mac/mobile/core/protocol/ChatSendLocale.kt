package bot.mac.mobile.core.protocol

import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/** Adds the locale captured when a user chat.send is first created. */
fun JsonObject.withClientLocale(localeTag: String): JsonObject = buildJsonObject {
    this@withClientLocale.forEach { (key, value) -> put(key, value) }
    put("client_locale", localeTag)
}
