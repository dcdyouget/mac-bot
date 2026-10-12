import { useEffect, useMemo, useRef, useState } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import { BarChart3, CalendarDays, ChevronDown, Download, TrendingDown, TrendingUp } from 'lucide-react'
import { invoke } from '@tauri-apps/api/core'
import { t, type CopyKey } from '../i18n'
import type { Json } from '../types'
import { getCalendarRange } from './dashboard.model'

type Range = 'today' | '7d' | '30d' | '90d'
type Dimension = 'model' | 'bot' | 'project'
type Metric = 'tokens' | 'cost' | 'requests'
type Totals = { input_tokens?: number; output_tokens?: number; cache_read_tokens?: number; cache_write_tokens?: number; requests?: number; cost?: number | null; tasks_done?: number }
type Summary = { current: Totals; previous: Totals }
type Day = { date: string; value: number; tokens: number; cost: number | null; requests: number; top_bot_id: string | null }
type Heatmap = { days?: Day[]; matrix?: number[][]; thresholds: [number, number, number] }
type Series = { key: string; label: string; values: number[]; input_values?: number[] | null; output_values?: number[] | null; total: number }
type Timeseries = { granularity: string; buckets: string[]; series: Series[] }
type UsageRow = { key: string; label: string; usage: Totals; sparkline: number[]; phases: Record<string, number> }
type Breakdown = { rows: UsageRow[] }
type Drill = { bot_id: string } | { project_id: string }

const RANGE_DAYS: Record<Range, number> = { today: 1, '7d': 7, '30d': 30, '90d': 90 }
const COLORS = ['--chart-1', '--chart-2', '--chart-3', '--chart-4', '--chart-5', '--chart-6']

export function DashboardPage() {
  const [range, setRange] = useState<Range>('30d')
  const [heatMode, setHeatMode] = useState<'calendar' | 'weekhour'>('calendar')
  const [dimension, setDimension] = useState<Dimension>('model')
  const [metric, setMetric] = useState<Metric>('tokens')
  const [summary, setSummary] = useState<Summary | null>(null)
  const [heatmap, setHeatmap] = useState<Heatmap | null>(null)
  const [timeseries, setTimeseries] = useState<Timeseries | null>(null)
  const [breakdown, setBreakdown] = useState<Breakdown | null>(null)
  const [drill, setDrill] = useState<Drill | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const windowRange = useMemo(() => getRange(range), [range])

  useEffect(() => {
    let alive = true
    setLoading(true)
    setError(null)
    const from = windowRange.from.toISOString()
    const to = windowRange.to.toISOString()
    const calendarRange = heatMode === 'calendar' ? getCalendarRange() : windowRange
    const request = (method: string, params: Record<string, Json>) => invoke<Json>('protocol_request', { method, params })
    void Promise.allSettled([
      request('usage.summary', { from, to }),
      request('usage.heatmap', { mode: heatMode, from: calendarRange.from.toISOString(), to: calendarRange.to.toISOString(), metric }),
      request('usage.timeseries', { from, to, granularity: 'auto', dimension, metric, split_io: false, top: 6 }),
      request('usage.breakdown', { from, to, dimension, ...(drill ?? {}) }),
    ]).then((results) => {
      if (!alive) return
      const [summaryResult, heatResult, seriesResult, breakdownResult] = results
      const failed = results.find((result) => result.status === 'rejected')
      setError(failed?.status === 'rejected' ? String(failed.reason) : null)
      if (summaryResult.status === 'fulfilled') setSummary(summaryResult.value as Summary)
      if (heatResult.status === 'fulfilled') setHeatmap(heatResult.value as Heatmap)
      if (seriesResult.status === 'fulfilled') setTimeseries(seriesResult.value as Timeseries)
      if (breakdownResult.status === 'fulfilled') setBreakdown(breakdownResult.value as Breakdown)
    }).finally(() => { if (alive) setLoading(false) })
    return () => { alive = false }
  }, [windowRange, heatMode, dimension, metric, drill])

  const totalTokens = Number(summary?.current.input_tokens ?? 0) + Number(summary?.current.output_tokens ?? 0)
  const previousTokens = Number(summary?.previous.input_tokens ?? 0) + Number(summary?.previous.output_tokens ?? 0)
  const cacheRate = totalTokens + Number(summary?.current.cache_read_tokens ?? 0) > 0
    ? Number(summary?.current.cache_read_tokens ?? 0) / (totalTokens + Number(summary?.current.cache_read_tokens ?? 0)) * 100
    : 0
  const metrics = [
    { key: 'tokens', label: t('dashboard.kpi.tokens'), value: shortNumber(totalTokens), current: totalTokens, previous: previousTokens },
    { key: 'cost', label: t('dashboard.kpi.cost'), value: formatCost(summary?.current.cost), current: Number(summary?.current.cost ?? 0), previous: Number(summary?.previous.cost ?? 0) },
    { key: 'requests', label: t('dashboard.kpi.requests'), value: Number(summary?.current.requests ?? 0).toLocaleString('zh-CN'), current: Number(summary?.current.requests ?? 0), previous: Number(summary?.previous.requests ?? 0) },
    { key: 'cache', label: t('dashboard.kpi.cache_hit'), value: `${Math.round(cacheRate)}%`, current: cacheRate, previous: null },
    { key: 'tasks', label: t('dashboard.kpi.tasks_done'), value: Number(summary?.current.tasks_done ?? 0).toLocaleString('zh-CN'), current: Number(summary?.current.tasks_done ?? 0), previous: Number(summary?.previous.tasks_done ?? 0) },
  ]

  async function exportCsv() {
    const rows = [['dimension', 'label', 'input_tokens', 'output_tokens', 'cache_read_tokens', 'requests', 'cost']]
    for (const row of breakdown?.rows ?? []) rows.push([dimension, row.label, String(row.usage.input_tokens ?? 0), String(row.usage.output_tokens ?? 0), String(row.usage.cache_read_tokens ?? 0), String(row.usage.requests ?? 0), String(row.usage.cost ?? 0)])
    const csv = rows.map((row) => row.map(escapeCsv).join(',')).join('\r\n')
    try {
      await invoke<boolean>('save_usage_csv', { fileName: `macbot-usage-${range}.csv`, csv: `\ufeff${csv}` })
    } catch (reason) {
      setError(String(reason))
    }
  }

  function selectDrill(row: UsageRow) {
    if (dimension === 'bot') setDrill((current) => current && 'bot_id' in current && current.bot_id === row.key ? null : { bot_id: row.key })
    else if (dimension === 'project') setDrill((current) => current && 'project_id' in current && current.project_id === row.key ? null : { project_id: row.key })
  }

  return <section className="desktop-page dashboard-page">
    <header className="desktop-page-toolbar glass-toolbar"><div className="desktop-page-title"><BarChart3 size={17} />{t('dashboard.title')}</div><div className="dashboard-toolbar-controls">
      <div className="segmented-control" role="tablist" aria-label={t('dashboard.title')}>
        {(['today', '7d', '30d', '90d'] as Range[]).map((value) => <button key={value} className={range === value ? 'selected' : ''} onClick={() => { setRange(value); setDrill(null) }}>{t(`dashboard.range.${value}` as CopyKey)}</button>)}
    </div><button className="dashboard-export" onClick={() => { void exportCsv() }}><Download size={13} />{t('dashboard.export_csv')}</button>
    </div></header>
    <div className="desktop-page-scroll dashboard-scroll">
      {error && <div className="dashboard-error" role="alert">{error}</div>}
      <div className="dashboard-kpis">{metrics.map((metricCard) => <div className="dashboard-kpi-card" key={metricCard.key} aria-busy={loading && !summary}>
        <span>{metricCard.label}</span><strong>{metricCard.value}</strong><small>{metricCard.previous === null ? '' : <><Delta current={metricCard.current} previous={metricCard.previous} /> {t('dashboard.kpi.vs_prev')}</>}</small>
      </div>)}</div>
      <section className="dashboard-card dashboard-heatmap-card">
        <header><strong>{t('dashboard.heatmap')}</strong><div className="segmented-control"><button className={heatMode === 'calendar' ? 'selected' : ''} onClick={() => setHeatMode('calendar')}><CalendarDays size={12} />{t('dashboard.heatmap.calendar')}</button><button className={heatMode === 'weekhour' ? 'selected' : ''} onClick={() => setHeatMode('weekhour')}>{t('dashboard.heatmap.week_hour')}</button></div></header>
        {heatMode === 'calendar' ? <CalendarHeatmap days={heatmap?.days ?? []} thresholds={heatmap?.thresholds ?? [0, 0, 0]} loading={loading && !heatmap} /> : <WeekHourHeatmap matrix={heatmap?.matrix ?? []} thresholds={heatmap?.thresholds ?? [0, 0, 0]} loading={loading && !heatmap} />}
        <div className="heatmap-legend"><span>{t('dashboard.heatmap.less')}</span>{[0, 1, 2, 3, 4].map((level) => <i key={level} className={`heat-level heat-${level}`} />)}<span>{t('dashboard.heatmap.more')}</span></div>
      </section>
      <section className="dashboard-card dashboard-timeseries-card">
        <header><strong>{t('dashboard.daily')}</strong><div className="dashboard-series-controls"><span>{t('dashboard.dimension')}</span><div className="segmented-control">{(['model', 'bot', 'project'] as Dimension[]).map((value) => <button key={value} className={dimension === value ? 'selected' : ''} onClick={() => { setDimension(value); setDrill(null) }}>{t(`dashboard.dimension.${value}` as CopyKey)}</button>)}</div><span>{t('dashboard.metric')}</span><div className="segmented-control">{(['tokens', 'cost', 'requests'] as Metric[]).map((value) => <button key={value} className={metric === value ? 'selected' : ''} onClick={() => setMetric(value)}>{metricLabel(value)}</button>)}</div></div></header>
        <div className="dashboard-chart-wrap"><TimeseriesChart data={timeseries} />
          <div className="dashboard-series-legend">{(timeseries?.series ?? []).map((series, index) => <div className="dashboard-series-item" key={series.key}><i style={{ background: `var(${COLORS[index % COLORS.length]})` }} /><span>{series.label}</span><b>{formatMetric(series.total, metric)}</b></div>)}</div>
        </div>
      </section>
      <section className="dashboard-card dashboard-breakdown-card">
        <header><strong>{dimensionLabel(dimension)}</strong>{drill && <button className="dashboard-clear-drill" onClick={() => setDrill(null)} aria-label={t('common.close')}><ChevronDown size={14} />{t('dashboard.drill.phases')}</button>}</header>
        <BreakdownList rows={breakdown?.rows ?? []} dimension={dimension} metric={metric} drill={drill} onDrill={selectDrill} />
        {drill && <div className="dashboard-phase-drill"><strong>{t('dashboard.drill.phases')}</strong>{breakdown?.rows.flatMap((row) => Object.entries(row.phases ?? {})).map(([phase, value]) => <span key={phase}>{phaseLabel(phase)}<b>{formatMetric(value, metric)}</b></span>)}</div>}
      </section>
    </div>
  </section>
}

function CalendarHeatmap({ days, thresholds, loading }: { days: Day[]; thresholds: [number, number, number]; loading: boolean }) {
  const first = days.length ? new Date(`${days[0].date}T00:00:00`) : new Date()
  const leading = (first.getDay() + 6) % 7
  const cells = [...Array(leading).fill(null), ...days]
  const weekCount = Math.ceil(cells.length / 7)
  return <div className="calendar-heatmap" aria-busy={loading}>
    <div className="calendar-weekdays">{Array.from({ length: 7 }, (_, index) => <span key={index}>{new Date(2024, 0, 1 + index).toLocaleDateString('zh-CN', { weekday: 'narrow' })}</span>)}</div>
    <div className="calendar-cells" style={{ gridTemplateColumns: `repeat(${weekCount}, 12px)` }}>{cells.map((day, index) => day
      ? <i key={day.date} className={`heat-level heat-${heatLevel(day.value, thresholds)}`} title={`${day.date} · ${formatMetric(day.value, 'tokens')}`} aria-label={`${day.date} · ${formatMetric(day.value, 'tokens')}`} />
      : <i key={`empty-${index}`} className="calendar-empty" />)}</div>
  </div>
}

function WeekHourHeatmap({ matrix, thresholds, loading }: { matrix: number[][]; thresholds: [number, number, number]; loading: boolean }) {
  const dayLabels = Array.from({ length: 7 }, (_, index) => new Date(2024, 0, 1 + index).toLocaleDateString('zh-CN', { weekday: 'short' }))
  return <div className="weekhour-heatmap" aria-busy={loading}><div className="weekhour-labels">{dayLabels.map((label) => <span key={label}>{label}</span>)}</div><div className="weekhour-grid">{Array.from({ length: 7 }, (_, day) => Array.from({ length: 24 }, (_, hour) => {
    const value = Number(matrix[day]?.[hour] ?? 0)
    return <i key={`${day}-${hour}`} className={`heat-level heat-${heatLevel(value, thresholds)}`} title={`${dayLabels[day]} ${hour}:00 · ${shortNumber(value)}`} />
  }))}</div><div className="weekhour-ticks"><span>0</span><span>6</span><span>12</span><span>18</span><span>24</span></div></div>
}

function TimeseriesChart({ data }: { data: Timeseries | null }) {
  const series = data?.series ?? []
  const values = series.flatMap((item) => item.values).filter(Number.isFinite)
  const max = Math.max(1, ...values)
  const width = 680
  const height = 180
  const points = Math.max(2, data?.buckets.length ?? 0)
  return <svg className="dashboard-line-chart" viewBox={`0 0 ${width} ${height}`} role="img" aria-label={t('dashboard.daily')}>
    {[0.25, 0.5, 0.75].map((fraction) => <line key={fraction} x1="0" x2={width} y1={height * fraction} y2={height * fraction} />)}
    {series.map((item, index) => {
      const path = item.values.map((value, point) => `${point === 0 ? 'M' : 'L'}${(point / Math.max(1, points - 1) * width).toFixed(1)} ${(height - Number(value) / max * height * 0.9 - height * 0.05).toFixed(1)}`).join(' ')
      return <path key={item.key} d={path} style={{ stroke: `var(${COLORS[index % COLORS.length]})` }} />
    })}
  </svg>
}

function BreakdownList({ rows, dimension, metric, drill, onDrill }: { rows: UsageRow[]; dimension: Dimension; metric: Metric; drill: Drill | null; onDrill: (row: UsageRow) => void }) {
  const parentRef = useRef<HTMLDivElement>(null)
  const virtualizer = useVirtualizer({ count: rows.length, getScrollElement: () => parentRef.current, estimateSize: () => 42, overscan: 6, useFlushSync: false })
  return <div className="dashboard-breakdown-table" ref={parentRef}>
    <div className="dashboard-breakdown-inner" style={{ height: `${virtualizer.getTotalSize()}px` }}>
      {virtualizer.getVirtualItems().map((item) => {
        const row = rows[item.index]
        const total = metric === 'tokens' ? Number(row.usage.input_tokens ?? 0) + Number(row.usage.output_tokens ?? 0) : metric === 'cost' ? Number(row.usage.cost ?? 0) : Number(row.usage.requests ?? 0)
        const selected = drill && ('bot_id' in drill ? drill.bot_id === row.key : drill.project_id === row.key)
        return <button className={`dashboard-breakdown-row ${selected ? 'selected' : ''}`} key={row.key} ref={virtualizer.measureElement} data-index={item.index} style={{ transform: `translateY(${item.start}px)` }} onClick={() => onDrill(row)} disabled={dimension === 'model'}>
          <span className="dashboard-breakdown-name">{row.label}</span><span className="dashboard-sparkline">{row.sparkline.map((value, index) => <i key={index} style={{ height: `${Math.max(2, Math.min(24, value / Math.max(1, ...row.sparkline) * 24))}px` }} />)}</span><strong>{formatMetric(total, metric)}</strong>
        </button>
      })}
    </div>
  </div>
}

function Delta({ current, previous }: { current: number; previous: number }) {
  if (!previous) return <span className="dashboard-delta neutral">—</span>
  const percent = (current - previous) / previous * 100
  const Icon = percent >= 0 ? TrendingUp : TrendingDown
  return <span className={`dashboard-delta ${percent >= 0 ? 'up' : 'down'}`}><Icon size={11} />{Math.abs(percent).toFixed(0)}%</span>
}
function getRange(range: Range) {
  const now = new Date()
  const to = new Date(now)
  const from = new Date(now)
  if (range === 'today') from.setHours(0, 0, 0, 0)
  else from.setDate(from.getDate() - RANGE_DAYS[range])
  return { from, to }
}
function heatLevel(value: number, thresholds: [number, number, number]) { return value <= 0 ? 0 : value <= thresholds[0] ? 1 : value <= thresholds[1] ? 2 : value <= thresholds[2] ? 3 : 4 }
function dimensionLabel(value: Dimension) { return t(`dashboard.dimension.${value}` as CopyKey) }
function phaseLabel(value: string) {
  const key: Record<string, CopyKey> = {
    chat: 'dashboard.phase.chat', work: 'dashboard.phase.work', subagent: 'dashboard.phase.subagent',
    coordinate: 'dashboard.phase.coordinate', memory: 'dashboard.phase.memory', compact: 'dashboard.phase.compact',
  }
  return key[value] ? t(key[value]) : value
}
function metricLabel(value: Metric) { return value === 'tokens' ? t('dashboard.kpi.tokens') : value === 'cost' ? t('dashboard.kpi.cost') : t('dashboard.kpi.requests') }
function formatMetric(value: number, metric: Metric) { return metric === 'cost' ? formatCost(value) : metric === 'requests' ? Math.round(value).toLocaleString('zh-CN') : shortNumber(value) }
function formatCost(value?: number | null) { return `¥${Number(value ?? 0).toFixed(2)}` }
function shortNumber(value: number) { return value >= 1_000_000 ? `${(value / 1_000_000).toFixed(1)}M` : value >= 10_000 ? `${(value / 1_000).toFixed(1)}k` : Math.round(value).toLocaleString('zh-CN') }
function escapeCsv(value: string) { return `"${value.replaceAll('"', '""')}"` }
