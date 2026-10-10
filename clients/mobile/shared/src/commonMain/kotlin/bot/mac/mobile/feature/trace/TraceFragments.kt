package bot.mac.mobile.feature.trace

internal fun visibleTraceFragments(
    fragments: Map<String, String>,
    requestId: String,
    callId: String,
    showThinking: Boolean,
    showOutput: Boolean,
): List<String> = fragments.filter { (key, value) ->
    val channel = key.substringAfterLast(':')
    val matches = when (channel) {
        "text" -> requestId.isNotBlank() && key.endsWith(":$requestId:text")
        "thinking" -> showThinking && requestId.isNotBlank() && key.endsWith(":$requestId:thinking")
        "tool" -> showOutput && callId.isNotBlank() && key.endsWith(":$callId:tool")
        else -> false
    }
    matches && value.isNotBlank()
}.values.toList()
