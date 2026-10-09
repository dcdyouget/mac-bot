package bot.mac.mobile.feature.dashboard

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.gestures.detectTapGestures
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
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material3.Card
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.protocol.arr
import bot.mac.mobile.core.protocol.long
import bot.mac.mobile.core.protocol.obj
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.platform.PickedFile
import bot.mac.mobile.core.platform.exportFile
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.resources.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import kotlin.time.Clock
import kotlin.time.Instant
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.jetbrains.compose.resources.stringResource

private enum class HeatMode { CALENDAR, WEEK_HOUR }
private enum class Dimension { MODEL, BOT, PROJECT }
private enum class Metric { TOKENS, COST, REQUESTS }
private enum class Range { TODAY, DAYS_7, DAYS_30, DAYS_90, CUSTOM }

private val Range.days: Long
    get() = when (this) {
        Range.TODAY -> 1L
        Range.DAYS_7 -> 7L
        Range.DAYS_30 -> 30L
        Range.DAYS_90 -> 90L
        Range.CUSTOM -> 30L
    }

@Composable
fun DashboardScreen(repository: MobileRepository, onBack: () -> Unit, onSelectHost: () -> Unit = {}) {
    var summary by remember { mutableStateOf<JsonObject?>(null) }
    var heatmap by remember { mutableStateOf<JsonObject?>(null) }
    var timeseries by remember { mutableStateOf<JsonObject?>(null) }
    var breakdown by remember { mutableStateOf<JsonObject?>(null) }
    var heatMode by remember { mutableStateOf(HeatMode.CALENDAR) }
    var dimension by remember { mutableStateOf(Dimension.MODEL) }
    var metric by remember { mutableStateOf(Metric.TOKENS) }
    var range by remember { mutableStateOf(Range.DAYS_30) }
    var customFrom by remember { mutableStateOf("") }
    var customTo by remember { mutableStateOf("") }
    var selectedDay by remember { mutableStateOf<String?>(null) }
    var splitIo by remember { mutableStateOf(false) }
    var visibleSeries by remember { mutableStateOf<Set<String>?>(null) }
    var csvPreview by remember { mutableStateOf<String?>(null) }
    var csvError by remember { mutableStateOf<String?>(null) }
    var drillBotId by remember { mutableStateOf<String?>(null) }
    var loading by remember { mutableStateOf(false) }
    var loadError by remember { mutableStateOf<String?>(null) }
    var reloadJob by remember { mutableStateOf<Job?>(null) }
    var reloadGeneration by remember { mutableStateOf(0) }
    val scope = rememberCoroutineScope()

    fun periodBounds(now: Instant): Pair<String, String> {
        if (range == Range.CUSTOM && customFrom.isNotBlank() && customTo.isNotBlank()) {
            return "${customFrom.trim()}T00:00:00Z" to "${customTo.trim()}T23:59:59Z"
        }
        val from = Instant.fromEpochMilliseconds(now.toEpochMilliseconds() - range.days * 86_400_000L).toString()
        return from to now.toString()
    }

    fun reload() {
        reloadJob?.cancel()
        val generation = reloadGeneration + 1
        reloadGeneration = generation
        reloadJob = scope.launch {
            loading = true
            loadError = null
            try {
                val now = Clock.System.now()
                val (rangeFrom, rangeTo) = periodBounds(now)
                val from = selectedDay?.let { "${it}T00:00:00Z" } ?: rangeFrom
                val to = selectedDay?.let { "${it}T23:59:59Z" } ?: rangeTo
                val period = buildJsonObject { put("from", from); put("to", to) }
                summary = repository.call("usage.summary", period)
                heatmap = repository.call("usage.heatmap", buildJsonObject {
                    put("mode", if (heatMode == HeatMode.CALENDAR) "calendar" else "weekhour")
                    put("from", rangeFrom); put("to", rangeTo)
                    put("metric", metric.name.lowercase())
                })
                timeseries = repository.call("usage.timeseries", buildJsonObject {
                    put("from", from); put("to", to)
                    put("granularity", "auto"); put("dimension", dimension.name.lowercase()); put("metric", metric.name.lowercase())
                    put("split_io", splitIo)
                    put("top", 6)
                })
                breakdown = repository.call("usage.breakdown", buildJsonObject {
                    put("from", from); put("to", to); put("dimension", dimension.name.lowercase())
                    drillBotId?.let { put("drill", buildJsonObject { put("bot_id", it) }) }
                })
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (failure: Throwable) {
                loadError = failure.message.orEmpty()
            } finally {
                if (reloadGeneration == generation) loading = false
            }
        }
    }

    LaunchedEffect(heatMode, dimension, metric, range, customFrom, customTo, selectedDay, splitIo) { reload() }
    LaunchedEffect(timeseries, splitIo) { visibleSeries = null }

    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = onBack) { Text("‹", style = MaterialTheme.typography.headlineSmall) }
            Text(stringResource(Res.string.dashboard_title), Modifier.weight(1f), style = MaterialTheme.typography.titleLarge)
            TextButton(onClick = onSelectHost) { Text(stringResource(Res.string.dashboard_host_select)) }
            IconButton(onClick = { reload() }) { Text("↻", color = MaterialTheme.colorScheme.primary) }
        }
        HorizontalDivider()
        if (loading) androidx.compose.material3.LinearProgressIndicator(Modifier.fillMaxWidth())
        loadError?.let { Text(stringResource(Res.string.dashboard_error, it), color = MaterialTheme.colorScheme.error, modifier = Modifier.padding(horizontal = 12.dp, vertical = 4.dp)) }
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
                if (range == Range.CUSTOM) {
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        OutlinedTextField(customFrom, { customFrom = it }, Modifier.weight(1f), label = { Text(stringResource(Res.string.dashboard_custom_from)) }, singleLine = true)
                        OutlinedTextField(customTo, { customTo = it }, Modifier.weight(1f), label = { Text(stringResource(Res.string.dashboard_custom_to)) }, singleLine = true)
                    }
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
                        FilterChip(dimension == candidate, { dimension = candidate; drillBotId = null }, label = { Text(dimensionLabel(candidate)) })
                    }
                }
                Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    Metric.entries.forEach { candidate ->
                        FilterChip(metric == candidate, { metric = candidate }, label = { Text(metricLabel(candidate)) })
                    }
                }
                if (metric == Metric.TOKENS) {
                    FilterChip(splitIo, { splitIo = !splitIo }, label = { Text(stringResource(Res.string.dashboard_split_io)) })
                }
                if (selectedDay != null) Text("${selectedDay}", style = MaterialTheme.typography.labelMedium)
                TimeseriesChart(timeseries, splitIo, metric, visibleSeries, onVisibleSeriesChanged = { visibleSeries = it })
            }
            item {
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                    SectionHeader(stringResource(Res.string.dashboard_detail), Modifier.weight(1f))
                    TextButton(onClick = {
                        scope.launch {
                            csvError = null
                            try {
                                val now = Clock.System.now()
                                val (rangeFrom, rangeTo) = periodBounds(now)
                                val from = selectedDay?.let { "${it}T00:00:00Z" } ?: rangeFrom
                                val to = selectedDay?.let { "${it}T23:59:59Z" } ?: rangeTo
                                val csv = repository.fetchText(
                                    "/api/v1/usage/export.csv",
                                    mapOf("from" to from, "to" to to, "dimension" to dimension.name.lowercase()),
                                )
                                val exported = try {
                                    exportFile(PickedFile("usage.csv", "text/csv", csv.encodeToByteArray()))
                                } catch (cancelled: CancellationException) {
                                    throw cancelled
                                } catch (_: Throwable) {
                                    false
                                }
                                if (!exported) {
                                    csvPreview = csv
                                }
                            } catch (cancelled: CancellationException) {
                                throw cancelled
                            } catch (failure: Throwable) {
                                csvError = failure.message.orEmpty()
                            }
                        }
                    }) { Text(stringResource(Res.string.dashboard_export_csv)) }
                }
                csvError?.let { Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall) }
                drillBotId?.let {
                    TextButton(onClick = { drillBotId = null; dimension = Dimension.BOT }) { Text(stringResource(Res.string.dashboard_back_to_all)) }
                }
                BreakdownList(breakdown, onDrill = { id ->
                    if (dimension == Dimension.BOT) {
                        drillBotId = id
                        dimension = Dimension.PROJECT
                    }
                })
            }
            item { Spacer(Modifier.height(12.dp)) }
        }
    }
    csvPreview?.let { csv ->
        AlertDialog(
            onDismissRequest = { csvPreview = null },
            title = { Text(stringResource(Res.string.dashboard_export_csv)) },
            text = {
                SelectionContainer {
                    Text(csv, Modifier.height(360.dp).verticalScroll(rememberScrollState()), style = MaterialTheme.typography.bodySmall)
                }
            },
            confirmButton = { TextButton(onClick = { csvPreview = null }) { Text(stringResource(Res.string.common_close)) } },
        )
    }
}

@Composable
private fun SummaryCards(summary: JsonObject?) {
    val current = summary?.obj("current")
    val previous = summary?.obj("previous")
    val currentCost = current?.number("cost")
    val previousCost = previous?.number("cost")
    Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        SummaryCard(stringResource(Res.string.dashboard_tokens), current?.long("input_tokens", "output_tokens") ?: current?.long("tokens") ?: 0L, previous?.long("input_tokens", "output_tokens") ?: previous?.long("tokens") ?: 0L)
        SummaryCard(stringResource(Res.string.dashboard_cost), currentCost?.let(::formatCost) ?: stringResource(Res.string.dashboard_unpriced), previousCost?.let(::formatCost) ?: stringResource(Res.string.dashboard_unpriced))
        SummaryCard(stringResource(Res.string.dashboard_requests), current?.long("requests") ?: 0L, previous?.long("requests") ?: 0L)
        SummaryCard(stringResource(Res.string.dashboard_cache_hit), formatPercent(cacheHitRatio(current)), formatPercent(cacheHitRatio(previous)))
        SummaryCard(stringResource(Res.string.dashboard_tasks), current?.long("tasks_done") ?: 0L, previous?.long("tasks_done") ?: 0L)
    }
}

@Composable
private fun SummaryCard(label: String, value: Any, previous: Any) {
    Card(Modifier.width(138.dp)) {
        Column(Modifier.padding(12.dp)) {
            Text(label, style = MaterialTheme.typography.labelMedium)
            Text(formatValue(value), style = MaterialTheme.typography.titleMedium)
            if (previous.toString().toDoubleOrNull()?.let { it > 0 } == true) Text("↔ ${formatValue(previous)}", style = MaterialTheme.typography.labelSmall)
        }
    }
}

@Composable
private fun SectionHeader(text: String, modifier: Modifier = Modifier) { Text(text, style = MaterialTheme.typography.titleMedium, modifier = modifier.padding(top = 4.dp)) }

@Composable
private fun Heatmap(data: JsonObject?, mode: HeatMode, onDaySelected: (String) -> Unit) {
    val cells: List<JsonObject> = if (mode == HeatMode.CALENDAR) {
        data?.arr("days").orEmpty().mapNotNull { it as? JsonObject }
    } else {
        data?.arr("matrix")?.toList().orEmpty().flatMapIndexed { row, rowElement ->
            (rowElement as? JsonArray)?.toList().orEmpty().mapIndexed { col, value ->
                buildJsonObject { put("date", "$row:${col + 1}"); put("value", value) }
            }
        }
    }
    if (cells.isEmpty()) { Text(stringResource(Res.string.dashboard_no_data), modifier = Modifier.padding(16.dp)); return }
    val thresholds = data?.arr("thresholds")?.mapNotNull { it.toString().trim('"').toDoubleOrNull() }.orEmpty()
    Box(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(vertical = 8.dp)) {
        Canvas(Modifier.width(if (mode == HeatMode.CALENDAR) 850.dp else 650.dp).height(if (mode == HeatMode.CALENDAR) 108.dp else 150.dp)) {
            val cell = 12.dp.toPx(); val gap = 3.dp.toPx(); val step = cell + gap
            cells.forEachIndexed { index, item ->
                val value = item.number("value") ?: 0.0
                val x: Int; val y: Int
                if (mode == HeatMode.CALENDAR) { x = index / 7; y = index % 7 } else { x = index % 24; y = index / 24 }
                drawRoundRect(heatColor(value, thresholds), Offset(x * step, y * step), androidx.compose.ui.geometry.Size(cell, cell), CornerRadius(2.dp.toPx()))
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
private fun TimeseriesChart(
    data: JsonObject?,
    splitIo: Boolean,
    metric: Metric,
    visibleSeries: Set<String>?,
    onVisibleSeriesChanged: (Set<String>?) -> Unit,
) {
    val series = data?.arr("series").orEmpty().mapNotNull { it as? JsonObject }
    val inputLabel = stringResource(Res.string.dashboard_input)
    val outputLabel = stringResource(Res.string.dashboard_output)
    val lines = series.flatMap { row ->
        val key = row.str("key") ?: row.str("label") ?: return@flatMap emptyList()
        if (!splitIo) listOf(ChartLine(key, row.str("label") ?: key, row.numbers("values"), 0))
        else listOfNotNull(
            row.numbersOrNull("input_values")?.let { ChartLine("$key:input", "${row.str("label") ?: key} · $inputLabel", it, 0) },
            row.numbersOrNull("output_values")?.let { ChartLine("$key:output", "${row.str("label") ?: key} · $outputLabel", it, 1) },
        )
    }
    val selectedSeries = visibleSeries
    val effectiveVisible = selectedSeries ?: lines.map { it.key }.toSet()
    val shown = lines.filter { it.key in effectiveVisible }
    val all = shown.flatMap { it.values }
    if (lines.isEmpty() || all.isEmpty()) { Text(stringResource(Res.string.dashboard_no_data), modifier = Modifier.padding(16.dp)); return }
    val max = all.maxOrNull()?.coerceAtLeast(1.0) ?: 1.0
    val bucketCount = lines.maxOfOrNull { it.values.size }?.coerceAtLeast(1) ?: 1
    val buckets = data?.arr("buckets").orEmpty()
    var selectedBucket by remember(data, splitIo, visibleSeries) { mutableStateOf<Int?>(null) }
    Column(Modifier.fillMaxWidth()) {
        Row(Modifier.fillMaxWidth().height(176.dp), verticalAlignment = Alignment.CenterVertically) {
            Column(
                Modifier.width(56.dp).fillMaxSize().padding(vertical = 4.dp),
                verticalArrangement = Arrangement.SpaceBetween,
                horizontalAlignment = Alignment.End,
            ) {
                Text(formatAxis(max, metric), style = MaterialTheme.typography.labelSmall, maxLines = 1, softWrap = false)
                Text(formatAxis(max / 2.0, metric), style = MaterialTheme.typography.labelSmall, maxLines = 1, softWrap = false)
                Text(formatAxis(0.0, metric), style = MaterialTheme.typography.labelSmall, maxLines = 1, softWrap = false)
            }
            Canvas(
                Modifier.weight(1f).fillMaxSize().padding(start = 6.dp, end = 4.dp, top = 4.dp, bottom = 4.dp)
                    .pointerInput(bucketCount) {
                        detectTapGestures { offset ->
                            selectedBucket = (offset.x / size.width * bucketCount).toInt().coerceIn(0, bucketCount - 1)
                        }
                    },
            ) {
                val chartMax = max
                val chartWidth = size.width
                val chartHeight = size.height
                drawLine(Color.LightGray.copy(alpha = 0.35f), Offset(0f, 0f), Offset(chartWidth, 0f))
                drawLine(Color.LightGray.copy(alpha = 0.35f), Offset(0f, chartHeight / 2f), Offset(chartWidth, chartHeight / 2f))
                drawLine(Color.LightGray.copy(alpha = 0.35f), Offset(0f, chartHeight), Offset(chartWidth, chartHeight))
            val colors = listOf(Color(0xFF5757D9), Color(0xFF0A9E72), Color(0xFFE1842A), Color(0xFFD64A5B), Color(0xFF7B61A8), Color(0xFF2B7BBC))
                shown.forEachIndexed { index, line ->
                    val values = line.values
                    if (values.size < 2) return@forEachIndexed
                    val path = Path(); values.forEachIndexed { point, value ->
                        val x = point * (chartWidth / (values.size - 1)); val y = chartHeight - (value / chartMax * chartHeight)
                        if (point == 0) path.moveTo(x, y.toFloat()) else path.lineTo(x, y.toFloat())
                    }
                    drawPath(
                        path,
                        colors[index % colors.size],
                        style = Stroke(
                            width = 3.dp.toPx(),
                            pathEffect = if (line.channel == 1) PathEffect.dashPathEffect(floatArrayOf(10f, 8f)) else null,
                        ),
                    )
                }
            }
        }
        if (bucketCount > 1) {
            Row(Modifier.fillMaxWidth().padding(start = 42.dp, end = 4.dp), horizontalArrangement = Arrangement.SpaceBetween) {
                Text(bucketLabel(buckets.getOrNull(0), 0), style = MaterialTheme.typography.labelSmall)
                Text(bucketLabel(buckets.getOrNull(bucketCount - 1), bucketCount - 1), style = MaterialTheme.typography.labelSmall)
            }
        }
        selectedBucket?.let { index ->
            Text(stringResource(Res.string.dashboard_bucket_detail, bucketLabel(buckets.getOrNull(index), index)), style = MaterialTheme.typography.labelMedium, modifier = Modifier.padding(top = 6.dp))
            shown.forEach { line ->
                line.values.getOrNull(index)?.let { value ->
                    Text(stringResource(Res.string.dashboard_bucket_value, line.label, formatMetricValue(value, metric)), style = MaterialTheme.typography.labelSmall)
                }
            }
        }
    }
    Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        lines.forEachIndexed { index, line ->
            FilterChip(
                selected = line.key in effectiveVisible,
                onClick = {
                    val next = when {
                        selectedSeries == null -> setOf(line.key)
                        selectedSeries.size == 1 && line.key in effectiveVisible -> null
                        else -> setOf(line.key)
                    }
                    onVisibleSeriesChanged(next)
                },
                label = { Text(line.label, style = MaterialTheme.typography.labelSmall) },
            )
        }
    }
}

private data class ChartLine(val key: String, val label: String, val values: List<Double>, val channel: Int)

@Composable
private fun BreakdownList(data: JsonObject?, onDrill: (String) -> Unit) {
    val rows = data?.arr("rows").orEmpty().mapNotNull { it as? JsonObject }
    if (rows.isEmpty()) { Text(stringResource(Res.string.dashboard_no_data), modifier = Modifier.padding(16.dp)); return }
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        rows.forEach { row ->
            val usage = row.obj("usage")
            Card(onClick = { onDrill(row.str("key")) }, modifier = Modifier.fillMaxWidth()) {
                Row(Modifier.fillMaxWidth().padding(12.dp), verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text(row.str("label").ifBlank { row.str("key") }, style = MaterialTheme.typography.titleSmall)
                        Text("${stringResource(Res.string.dashboard_detail_input)} ${usage.long("input_tokens")} · ${stringResource(Res.string.dashboard_detail_output)} ${usage.long("output_tokens")} · ${stringResource(Res.string.dashboard_detail_cache_read)} ${usage.long("cache_read_tokens")} · ${usage.long("requests")}", style = MaterialTheme.typography.bodySmall)
                        val sparkline = row.arr("sparkline").mapNotNull { it.toString().trim('"').toDoubleOrNull() }
                        if (sparkline.size > 1) Sparkline(sparkline)
                        val phases = row.obj("phases")
                        if (phases.isNotEmpty()) {
                            val phaseLabels = mapOf(
                                "chat" to stringResource(Res.string.dashboard_phase_chat),
                                "work" to stringResource(Res.string.dashboard_phase_work),
                                "subagent" to stringResource(Res.string.dashboard_phase_subagent),
                                "coordinate" to stringResource(Res.string.dashboard_phase_coordinate),
                                "memory" to stringResource(Res.string.dashboard_phase_memory),
                                "compact" to stringResource(Res.string.dashboard_phase_compact),
                            )
                            Text("${stringResource(Res.string.dashboard_detail_phases)}: ${phases.entries.joinToString(" · ") { "${phaseLabels[it.key] ?: it.key} ${it.value}" }}", style = MaterialTheme.typography.labelSmall)
                        }
                    }
                    Text(usage.number("cost")?.let(::formatCost) ?: stringResource(Res.string.dashboard_unpriced), style = MaterialTheme.typography.labelLarge)
                }
            }
        }
    }
}

@Composable
private fun Sparkline(values: List<Double>) {
    val primary = MaterialTheme.colorScheme.primary
    Canvas(Modifier.width(88.dp).height(28.dp).padding(vertical = 4.dp)) {
        val min = values.minOrNull() ?: return@Canvas
        val max = values.maxOrNull() ?: return@Canvas
        val span = (max - min).coerceAtLeast(1e-9)
        val path = Path()
        values.forEachIndexed { index, value ->
            val x = if (values.size == 1) 0f else index * size.width / (values.size - 1)
            val y = size.height - ((value - min) / span * size.height).toFloat()
            if (index == 0) path.moveTo(x, y) else path.lineTo(x, y)
        }
        drawPath(path, primary, style = Stroke(width = 2.dp.toPx()))
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

private fun cacheHitRatio(value: JsonObject?): Double = value?.let {
    val read = it.long("cache_read_tokens")?.toDouble() ?: 0.0
    val input = it.long("input_tokens")?.toDouble() ?: 0.0
    if (read + input <= 0.0) 0.0 else read / (read + input)
} ?: 0.0

private fun formatPercent(value: Double): String = "${(value * 100.0).toInt()}%"
private fun formatPercent(value: String): String = value

private fun formatCost(value: Double): String {
    val rounded = kotlin.math.round(value * 100.0) / 100.0
    val text = rounded.toString()
    return if (text.contains('.')) text.substringBefore('.') + "." + text.substringAfter('.').padEnd(2, '0').take(2)
    else "$text.00"
}

private fun formatMetricValue(value: Double, metric: Metric): String = if (metric == Metric.COST) formatCost(value) else if (value == value.toLong().toDouble()) value.toLong().toString() else formatValue(value)
private fun formatAxis(value: Double, metric: Metric): String = formatMetricValue(value, metric)

private fun bucketLabel(value: JsonElement?, fallback: Int): String {
    val label = when (value) {
        is JsonObject -> listOf("label", "start", "date", "time").firstNotNullOfOrNull { key -> value.str(key).takeIf { it.isNotBlank() } }
        null -> null
        else -> value.toString().trim('"').takeIf { it.isNotBlank() }
    }
    return label?.let {
        if (it.length >= 16 && it[10] == 'T') it.take(10) + " " + it.substring(11, 16) else it
    } ?: (fallback + 1).toString()
}

private fun Dimension.label(): String = name.lowercase()
@Composable private fun dimensionLabel(value: Dimension): String = when (value) {
    Dimension.MODEL -> stringResource(Res.string.dashboard_model)
    Dimension.BOT -> stringResource(Res.string.dashboard_bot)
    Dimension.PROJECT -> stringResource(Res.string.dashboard_project)
}
@Composable private fun metricLabel(value: Metric): String = when (value) {
    Metric.TOKENS -> stringResource(Res.string.dashboard_tokens)
    Metric.COST -> stringResource(Res.string.dashboard_cost)
    Metric.REQUESTS -> stringResource(Res.string.dashboard_requests)
}
@Composable private fun Range.label(): String = when (this) {
    Range.TODAY -> stringResource(Res.string.dashboard_today)
    Range.DAYS_7 -> stringResource(Res.string.dashboard_seven_days)
    Range.DAYS_30 -> stringResource(Res.string.dashboard_thirty_days)
    Range.DAYS_90 -> stringResource(Res.string.dashboard_ninety_days)
    Range.CUSTOM -> stringResource(Res.string.dashboard_custom)
}
private fun formatValue(value: Any): String = when (value) {
    is Double -> value.toString().let { if (it.length > 10) it.take(10) else it }
    is Float -> value.toString().let { if (it.length > 10) it.take(10) else it }
    else -> value.toString()
}

private fun JsonObject.long(vararg keys: String): Long? = keys.mapNotNull { key -> this[key]?.toString()?.trim('"')?.toLongOrNull() }.sum().takeIf { it > 0 }
private fun JsonObject.number(key: String): Double? = this[key]?.toString()?.trim('"')?.toDoubleOrNull()
private fun JsonObject.numbers(key: String): List<Double> = this[key].let { value ->
    (value as? JsonArray).orEmpty().mapNotNull { it.toString().trim('"').toDoubleOrNull() }
}
private fun JsonObject.numbersOrNull(key: String): List<Double>? = if (this[key] is JsonArray) numbers(key) else null
private fun JsonArray?.orEmpty(): List<kotlinx.serialization.json.JsonElement> = this?.toList().orEmpty()
