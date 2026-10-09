package bot.mac.mobile.feature.dashboard

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.Card
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.protocol.arr
import bot.mac.mobile.core.protocol.long
import bot.mac.mobile.core.protocol.obj
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.resources.Res
import bot.mac.mobile.resources.dashboard_calendar
import bot.mac.mobile.resources.dashboard_cost
import bot.mac.mobile.resources.dashboard_detail
import bot.mac.mobile.resources.dashboard_no_data
import bot.mac.mobile.resources.dashboard_requests
import bot.mac.mobile.resources.dashboard_tasks
import bot.mac.mobile.resources.dashboard_heatmap
import bot.mac.mobile.resources.dashboard_title
import bot.mac.mobile.resources.dashboard_tokens
import bot.mac.mobile.resources.dashboard_trend
import bot.mac.mobile.resources.dashboard_weekhour
import bot.mac.mobile.resources.dashboard_range
import bot.mac.mobile.resources.dashboard_today
import bot.mac.mobile.resources.dashboard_seven_days
import bot.mac.mobile.resources.dashboard_thirty_days
import bot.mac.mobile.resources.dashboard_ninety_days
import kotlinx.coroutines.launch
import kotlinx.datetime.Clock
import kotlinx.datetime.Instant
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.jetbrains.compose.resources.stringResource

private enum class HeatMode { CALENDAR, WEEK_HOUR }
private enum class Dimension { MODEL, BOT, PROJECT }
private enum class Metric { TOKENS, COST, REQUESTS }
private enum class Range { TODAY, DAYS_7, DAYS_30, DAYS_90 }

private val Range.days: Long
    get() = when (this) {
        Range.TODAY -> 1L
        Range.DAYS_7 -> 7L
        Range.DAYS_30 -> 30L
        Range.DAYS_90 -> 90L
    }

@Composable
fun DashboardScreen(repository: MobileRepository, onBack: () -> Unit) {
    var summary by remember { mutableStateOf<JsonObject?>(null) }
    var heatmap by remember { mutableStateOf<JsonObject?>(null) }
    var timeseries by remember { mutableStateOf<JsonObject?>(null) }
    var breakdown by remember { mutableStateOf<JsonObject?>(null) }
    var heatMode by remember { mutableStateOf(HeatMode.CALENDAR) }
    var dimension by remember { mutableStateOf(Dimension.MODEL) }
    var metric by remember { mutableStateOf(Metric.TOKENS) }
    var range by remember { mutableStateOf(Range.DAYS_30) }
    var selectedDay by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()

    fun reload() {
        scope.launch {
            val now = Clock.System.now()
            val to = now.toString()
            val from = selectedDay?.let { "${it}T00:00:00Z" }
                ?: Instant.fromEpochMilliseconds(now.toEpochMilliseconds() - range.days * 86_400_000L).toString()
            val period = buildJsonObject { put("from", from); put("to", to) }
            summary = repository.call("usage.summary", period)
            heatmap = repository.call("usage.heatmap", buildJsonObject {
                put("mode", if (heatMode == HeatMode.CALENDAR) "calendar" else "weekhour")
                put("from", from); put("to", to)
                put("metric", metric.name.lowercase())
            })
            timeseries = repository.call("usage.timeseries", buildJsonObject {
                put("from", from); put("to", to)
                put("granularity", "auto"); put("dimension", dimension.name.lowercase()); put("metric", metric.name.lowercase())
                put("top", 6)
            })
            breakdown = repository.call("usage.breakdown", buildJsonObject {
                put("from", from); put("to", to); put("dimension", dimension.name.lowercase())
            })
        }
    }

    LaunchedEffect(heatMode, dimension, metric, range, selectedDay) { reload() }

    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = onBack) { Text("‹", style = MaterialTheme.typography.headlineSmall) }
            Text(stringResource(Res.string.dashboard_title), Modifier.weight(1f), style = MaterialTheme.typography.titleLarge)
            IconButton(onClick = { reload() }) { Text("↻", color = MaterialTheme.colorScheme.primary) }
        }
        HorizontalDivider()
        androidx.compose.foundation.lazy.LazyColumn(
            Modifier.fillMaxSize().padding(horizontal = 12.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            item { SummaryCards(summary) }
            item {
                Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text(stringResource(Res.string.dashboard_range), style = MaterialTheme.typography.labelMedium)
                    Range.entries.forEach { candidate -> FilterChip(range == candidate, { range = candidate; selectedDay = null }, label = { Text(candidate.label()) }) }
                }
            }
            item {
                SectionHeader(stringResource(Res.string.dashboard_heatmap))
                Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    FilterChip(heatMode == HeatMode.CALENDAR, { heatMode = HeatMode.CALENDAR }, label = { Text(stringResource(Res.string.dashboard_calendar)) })
                    FilterChip(heatMode == HeatMode.WEEK_HOUR, { heatMode = HeatMode.WEEK_HOUR }, label = { Text(stringResource(Res.string.dashboard_weekhour)) })
                }
                Heatmap(data = heatmap, mode = heatMode, onDaySelected = { selectedDay = it })
            }
            item {
                SectionHeader(stringResource(Res.string.dashboard_trend))
                Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    Dimension.entries.forEach { candidate ->
                        FilterChip(dimension == candidate, { dimension = candidate }, label = { Text(dimensionLabel(candidate)) })
                    }
                }
                Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    Metric.entries.forEach { candidate ->
                        FilterChip(metric == candidate, { metric = candidate }, label = { Text(metricLabel(candidate)) })
                    }
                }
                if (selectedDay != null) Text("${selectedDay}", style = MaterialTheme.typography.labelMedium)
                TimeseriesChart(timeseries)
            }
            item {
                SectionHeader(stringResource(Res.string.dashboard_detail))
                BreakdownList(breakdown, metric, repository)
            }
            item { Spacer(Modifier.height(12.dp)) }
        }
    }
}

@Composable
private fun SummaryCards(summary: JsonObject?) {
    val current = summary?.obj("current")
    val previous = summary?.obj("previous")
    Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        SummaryCard(stringResource(Res.string.dashboard_tokens), current?.long("input_tokens", "output_tokens") ?: current?.long("tokens") ?: 0L, previous?.long("tokens") ?: 0L)
        SummaryCard(stringResource(Res.string.dashboard_cost), current?.number("cost") ?: 0.0, previous?.number("cost") ?: 0.0)
        SummaryCard(stringResource(Res.string.dashboard_requests), current?.long("requests") ?: 0L, previous?.long("requests") ?: 0L)
        SummaryCard(stringResource(Res.string.dashboard_tasks), current?.long("tasks_done") ?: 0L, previous?.long("tasks_done") ?: 0L)
    }
}

@Composable
private fun SummaryCard(label: String, value: Any, previous: Long) {
    Card(Modifier.width(138.dp)) {
        Column(Modifier.padding(12.dp)) {
            Text(label, style = MaterialTheme.typography.labelMedium)
            Text(formatValue(value), style = MaterialTheme.typography.titleMedium)
            if (previous > 0) Text("↔ ${formatValue(previous)}", style = MaterialTheme.typography.labelSmall)
        }
    }
}

@Composable
private fun SectionHeader(text: String) { Text(text, style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(top = 4.dp)) }

@Composable
private fun Heatmap(data: JsonObject?, mode: HeatMode, onDaySelected: (String) -> Unit) {
    val cells = if (mode == HeatMode.CALENDAR) data?.arr("days").orEmpty() else {
        data?.arr("matrix")?.flatMapIndexed { row, values -> values.mapIndexed { col, value -> buildJsonObject { put("date", "$row:${col + 1}"); put("value", value) } } } ?: emptyList()
    }
    if (cells.isEmpty()) { Text(stringResource(Res.string.dashboard_no_data), modifier = Modifier.padding(16.dp)); return }
    val thresholds = data?.arr("thresholds")?.mapNotNull { it.toString().trim('"').toDoubleOrNull() }.orEmpty()
    Box(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(vertical = 8.dp)) {
        Canvas(Modifier.width(if (mode == HeatMode.CALENDAR) 850.dp else 650.dp).height(if (mode == HeatMode.CALENDAR) 108.dp else 150.dp)) {
            val cell = 12.dp.toPx(); val gap = 3.dp.toPx(); val step = cell + gap
            cells.forEachIndexed { index, item ->
                val value = item.obj("value")?.number("value") ?: item.number("value") ?: 0.0
                val x: Int; val y: Int
                if (mode == HeatMode.CALENDAR) { x = index / 7; y = index % 7 } else { x = index % 24; y = index / 24 }
                drawRoundRect(heatColor(value, thresholds), Offset(x * step, y * step), androidx.compose.ui.geometry.Size(cell, cell), 2.dp.toPx())
            }
        }
    }
    if (mode == HeatMode.CALENDAR) {
        androidx.compose.foundation.lazy.LazyRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            items(cells.size) { index ->
                val day = cells[index].str("date") ?: return@items
                FilterChip(false, { onDaySelected(day) }, label = { Text(day.takeLast(5), style = MaterialTheme.typography.labelSmall) })
            }
        }
    }
}

@Composable
private fun TimeseriesChart(data: JsonObject?) {
    val series = data?.arr("series").orEmpty().mapNotNull { it as? JsonObject }
    val all = series.flatMap { it.arr("values").orEmpty().mapNotNull { value -> value.toString().trim('"').toDoubleOrNull() } }
    if (series.isEmpty() || all.isEmpty()) { Text(stringResource(Res.string.dashboard_no_data), modifier = Modifier.padding(16.dp)); return }
    val max = all.maxOrNull()?.coerceAtLeast(1.0) ?: 1.0
    Box(Modifier.fillMaxWidth().height(190.dp).horizontalScroll(rememberScrollState())) {
        Canvas(Modifier.width((series.first().arr("values")?.size ?: 8) * 56.dp).fillMaxSize().padding(8.dp)) {
            val colors = listOf(Color(0xFF5757D9), Color(0xFF0A9E72), Color(0xFFE1842A), Color(0xFFD64A5B), Color(0xFF7B61A8), Color(0xFF2B7BBC))
            series.forEachIndexed { index, row ->
                val values = row.arr("values").orEmpty().mapNotNull { it.toString().trim('"').toDoubleOrNull() }
                if (values.size < 2) return@forEachIndexed
                val path = Path(); values.forEachIndexed { point, value ->
                    val x = point * (size.width / (values.size - 1)); val y = size.height - (value / max * size.height)
                    if (point == 0) path.moveTo(x, y.toFloat()) else path.lineTo(x, y.toFloat())
                }
                drawPath(path, colors[index % colors.size], style = Stroke(width = 3.dp.toPx()))
            }
        }
    }
    Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        series.forEachIndexed { index, row -> Text("● ${row.str("label") ?: row.str("key") ?: ""}", color = listOf(Color(0xFF5757D9), Color(0xFF0A9E72), Color(0xFFE1842A))[index % 3], style = MaterialTheme.typography.labelSmall) }
    }
}

@Composable
private fun BreakdownList(data: JsonObject?, metric: Metric, repository: MobileRepository) {
    val rows = data?.arr("rows").orEmpty().mapNotNull { it as? JsonObject }
    if (rows.isEmpty()) { Text(stringResource(Res.string.dashboard_no_data), modifier = Modifier.padding(16.dp)); return }
    androidx.compose.foundation.lazy.LazyColumn(verticalArrangement = Arrangement.spacedBy(4.dp), userScrollEnabled = false) {
        items(rows.size) { index ->
            val row = rows[index]; val usage = row.obj("usage")
            Card(Modifier.fillMaxWidth()) {
                Row(Modifier.fillMaxWidth().padding(12.dp), verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text(row.str("label") ?: row.str("key") ?: "", style = MaterialTheme.typography.titleSmall)
                        Text("${usage?.long("input_tokens") ?: 0} + ${usage?.long("output_tokens") ?: 0} · ${usage?.long("requests") ?: 0}", style = MaterialTheme.typography.bodySmall)
                    }
                    Text(formatValue(usage?.number("cost") ?: 0.0), style = MaterialTheme.typography.labelLarge)
                }
            }
        }
    }
}

internal fun heatmapBucket(value: Double, thresholds: List<Double>): Int = when {
    value <= 0 -> 0
    thresholds.getOrNull(0)?.let { value <= it } == true -> 1
    thresholds.getOrNull(1)?.let { value <= it } == true -> 2
    thresholds.getOrNull(2)?.let { value <= it } == true -> 3
    else -> 4
}
private fun heatColor(value: Double, thresholds: List<Double>): Color = when (heatmapBucket(value, thresholds)) {
    0 -> Color(0xFFECECF2)
    1 -> Color(0xFFC9C9F3)
    2 -> Color(0xFF9292E8)
    3 -> Color(0xFF5757D9)
    else -> Color(0xFF292996)
}

private fun Dimension.label(): String = name.lowercase()
private fun dimensionLabel(value: Dimension): String = value.label().replaceFirstChar { it.uppercase() }
private fun metricLabel(value: Metric): String = value.name.lowercase()
@Composable private fun Range.label(): String = when (this) {
    Range.TODAY -> stringResource(Res.string.dashboard_today)
    Range.DAYS_7 -> stringResource(Res.string.dashboard_seven_days)
    Range.DAYS_30 -> stringResource(Res.string.dashboard_thirty_days)
    Range.DAYS_90 -> stringResource(Res.string.dashboard_ninety_days)
}
private fun formatValue(value: Any): String = when (value) {
    is Double -> value.toString().let { if (it.length > 10) it.take(10) else it }
    is Float -> value.toString().let { if (it.length > 10) it.take(10) else it }
    else -> value.toString()
}

private fun JsonObject.long(vararg keys: String): Long? = keys.mapNotNull { key -> this[key]?.toString()?.trim('"')?.toLongOrNull() }.sum().takeIf { it > 0 }
private fun JsonObject.number(key: String): Double? = this[key]?.toString()?.trim('"')?.toDoubleOrNull()
private fun JsonArray?.orEmpty(): List<kotlinx.serialization.json.JsonElement> = this?.toList().orEmpty()
