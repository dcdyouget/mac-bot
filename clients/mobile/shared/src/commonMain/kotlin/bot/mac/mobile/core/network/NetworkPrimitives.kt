package bot.mac.mobile.core.network

import io.ktor.http.URLBuilder
import io.ktor.http.URLProtocol
import io.ktor.http.appendPathSegments
import kotlin.math.max
import kotlin.math.min
import kotlin.random.Random
import kotlin.uuid.Uuid

/** Converts a user-entered host, HTTP URL, or WS URL to a protocol endpoint. */
object EndpointBuilder {
    fun main(address: String): String = websocketEndpoint(address, listOf("ws"))

    fun screen(address: String, botId: String, quality: String, tabId: String?): String {
        val builder = websocketBuilder(address)
        appendRoute(builder, listOf("ws", "screen"))
        builder.parameters.append("bot_id", botId)
        builder.parameters.append("quality", quality)
        if (tabId != null) builder.parameters.append("tab_id", tabId)
        return builder.buildString()
    }

    /** Builds an HTTP(S) endpoint while preserving an optional reverse-proxy path prefix. */
    fun http(address: String, path: String): String {
        val builder = parsedBuilder(address)
        builder.protocol = when (builder.protocol.name) {
            "https", "wss" -> URLProtocol.HTTPS
            else -> URLProtocol.HTTP
        }
        appendRoute(builder, pathSegments(path))
        return builder.buildString()
    }

    private fun websocketEndpoint(address: String, route: List<String>): String =
        websocketBuilder(address).also { appendRoute(it, route) }.buildString()

    private fun websocketBuilder(address: String): URLBuilder {
        val builder = parsedBuilder(address)
        builder.protocol = when (builder.protocol.name) {
            "https", "wss" -> URLProtocol.WSS
            else -> URLProtocol.WS
        }
        return builder
    }

    private fun parsedBuilder(address: String): URLBuilder {
        val raw = address.trim()
        require(raw.isNotEmpty()) { "address must not be empty" }
        val schemeSeparator = raw.indexOf("://")
        val normalized = if (schemeSeparator < 0) {
            "http://$raw"
        } else {
            val scheme = raw.substring(0, schemeSeparator).lowercase()
            require(scheme == "http" || scheme == "https" || scheme == "ws" || scheme == "wss") {
                "unsupported address scheme: $scheme"
            }
            scheme + raw.substring(schemeSeparator)
        }
        return URLBuilder(normalized)
    }

    private fun appendRoute(builder: URLBuilder, route: List<String>) {
        if (route.isEmpty()) return
        val current = builder.build().encodedPath.trim('/').split('/').filter { it.isNotEmpty() }
        val append = when {
            current.takeLast(route.size) == route -> emptyList()
            route == listOf("ws", "screen") && current.lastOrNull() == "ws" -> listOf("screen")
            else -> route
        }
        if (append.isNotEmpty()) builder.appendPathSegments(append)
    }

    private fun pathSegments(path: String): List<String> =
        path.substringBefore('?').trim('/').split('/').filter { it.isNotEmpty() }
}

class BackoffPolicy(
    private val random: Random = Random.Default,
    private val jitterRatio: Double = 0.2,
) {
    fun delayMillis(attempt: Int): Long {
        val exponent = min(max(attempt, 0), 5)
        val base = min(30_000L, 1_000L shl exponent)
        val jitter = (random.nextDouble(-jitterRatio, jitterRatio) * base).toLong()
        return (base + jitter).coerceIn(0L, 30_000L)
    }
}

fun nextAddressIndex(addressCount: Int, current: Int): Int =
    if (addressCount == 0) 0 else (current + 1) % addressCount

interface IdGenerator { fun nextId(): String }

object RandomIdGenerator : IdGenerator {
    override fun nextId(): String = Uuid.random().toString()
}
