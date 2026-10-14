import { useEffect, useMemo, useRef, useState } from 'react'
import {
  Activity, AlertCircle, ArrowLeft, AtSign, Brain, Check, ChevronRight,
  CircleAlert, Clock3, FileText, Layers3, LoaderCircle, Monitor, Search, Square,
  Terminal, Wrench, XCircle,
} from 'lucide-react'
import './TracePage.css'

export type TraceCopyKey =
  | 'trace.instruction' | 'trace.process' | 'trace.follow_latest' | 'trace.back_to_latest'
  | 'trace.thinking_summary' | 'trace.full_log' | 'trace.at_it' | 'trace.screen'
  | 'trace.filter.model' | 'trace.filter.tools' | 'trace.filter.subagents' | 'trace.filter.steer'
  | 'trace.search' | 'trace.item.start' | 'trace.item.model_request' | 'trace.item.subagents'
  | 'trace.item.steer' | 'trace.item.steer_applied' | 'trace.summary' | 'trace.running'
  | 'trace.queue.title' | 'trace.queue.hint' | 'trace.queue.to_steer' | 'trace.queue.seen'
  | 'trace.item.queue_review' | 'trace.queue.review_message' | 'trace.queue.review_result' | 'trace.queue.review_details'
  | 'trace.state.loading' | 'trace.state.empty' | 'trace.state.no_matches'
  | 'trace.state.error' | 'trace.state.load_more' | 'trace.state.unknown' | 'trace.state.instruction_empty'
  | 'trace.state.request_context' | 'trace.state.output' | 'trace.state.thinking' | 'trace.state.tool_failed'
  | 'trace.state.run_failed' | 'trace.state.returned' | 'trace.state.waiting' | 'trace.state.resumed'
  | 'trace.state.compaction' | 'trace.state.subagent_task' | 'trace.state.from'
  | 'common.stop' | 'common.retry' | 'status.working' | 'status.queued' | 'status.waiting_user'
  | 'status.blocked' | 'status.done' | 'status.failed' | 'status.cancelled'

export type TraceAssignment = {
  id: string
  project_id?: string | null
  origin_chat_id?: string
  bot_id?: string
  title?: string
  instruction?: string
  status?: string
  created_at?: string
  started_at?: string | null
  finished_at?: string | null
  pending_messages?: number
  steers?: { text?: string; at?: string; from?: { kind?: string; bot_id?: string } }[]
  usage?: { input_tokens?: number; output_tokens?: number; total_tokens?: number; cost?: number }
  model?: { provider?: string; model_id?: string; [key: string]: unknown }
}

export type TraceItem = {
  assignment_id?: string | null
  chat_id: string
  run_id: string
  aseq: number
  at: string
  type: string
  data: Record<string, unknown>
}

export type TraceQueueMessage = {
  id: string
  text: string
  sender: string
  at?: string
  seen?: boolean
  steerable?: boolean
}

export type TraceInFlight = { request_id: string; text: string; thinking: string; tool_args?: string }

export type TraceFilter = 'all' | 'model' | 'tools' | 'subagents' | 'steer'

export type TracePageProps = {
  assignment: TraceAssignment
  projectName?: string
  botName?: string
  items: TraceItem[]
  pendingMessages?: TraceQueueMessage[]
  inFlight?: TraceInFlight[]
  loading?: boolean
  loadingMore?: boolean
  error?: string | null
  hasMoreBefore?: boolean
  live?: boolean
  queueReviewDetails?: Record<string, { text: string; result?: string }>
  toolOutputs?: Record<string, string>
  onBack: () => void
  onLoadMore?: () => void
  onRetry?: () => void
  onStop?: (assignmentId: string) => void
  onMention?: (botId: string, chatId: string) => void
  onScreen?: (assignmentId: string) => void
  onQueueSteer?: (messageId: string) => void
  onOpenThinking?: (item: TraceItem) => void
  onOpenFullLog?: (assignmentId: string) => void
  t: (key: TraceCopyKey) => string
}

type TimelineEntry = { item: TraceItem; kind: TraceFilter | 'all'; title: string; detail?: string; result?: string; icon: typeof Activity; failed?: boolean; subagent?: boolean }

export function TracePage(props: TracePageProps) {
  const {
    assignment, projectName, botName, items, pendingMessages = [], inFlight = [], loading = false,
    loadingMore = false, error, hasMoreBefore = false, live = false, onBack, onLoadMore,
    onRetry, onStop, onMention, onScreen, onQueueSteer, onOpenThinking,
    onOpenFullLog, queueReviewDetails = {}, t,
  } = props
  const toolOutputs = props.toolOutputs ?? {}
  const [filter, setFilter] = useState<TraceFilter>('all')
  const [search, setSearch] = useState('')
  const [followLatest, setFollowLatest] = useState(true)
  const scrollRef = useRef<HTMLDivElement>(null)
  const stopStatus = ['working', 'waiting_user', 'waiting_bot', 'suspended'].includes(assignment.status ?? '')
  const latestThinkingItem = [...items].reverse().find((item) => item.type === 'llm.response' && stringValue(itemData(item).thinking))

  const entries = useMemo(() => {
    const requestNumbers = new Map<string, number>()
    const requestModels = new Map<string, string>()
    const toolNames = new Map<string, string>()
    let requestIndex = 0
    for (const item of items) {
      const data = itemData(item)
      if (item.type === 'llm.request') {
        requestIndex += 1
        const requestId = stringValue(data.request_id)
        if (requestId) {
          requestNumbers.set(requestId, requestIndex)
          requestModels.set(requestId, modelName(objectValue(data.model) as TraceAssignment['model']))
        }
      }
      if (item.type === 'tool.start') {
        const callId = stringValue(data.call_id)
        if (callId) toolNames.set(callId, stringValue(data.name))
      }
    }
    return items.map((item) => {
      const data = itemData(item)
      const requestId = stringValue(data.request_id)
      const callId = stringValue(data.call_id)
      return toTimelineEntry(item, t, requestNumbers.get(requestId) ?? requestIndex, requestModels.get(requestId), toolNames.get(callId))
    })
  }, [items, t])
  const visibleEntries = useMemo(() => {
    const query = search.trim().toLocaleLowerCase()
    return entries.filter((entry) => {
      const filterMatch = filter === 'all' || entry.kind === filter || (filter === 'subagents' && entry.subagent === true)
      const queryMatch = !query || `${entry.title} ${entry.detail ?? ''} ${entry.result ?? ''} ${JSON.stringify(itemData(entry.item))}`.toLocaleLowerCase().includes(query)
      return filterMatch && queryMatch
    })
  }, [entries, filter, search])

  useEffect(() => {
    if (followLatest && scrollRef.current) scrollRef.current.scrollTop = scrollRef.current.scrollHeight
  }, [followLatest, visibleEntries.length, inFlight.length])

  const usage = assignment.usage ?? {}
  const usageInput = numberValue(usage.input_tokens)
  const usageOutput = numberValue(usage.output_tokens)
  const usageTotal = numberValue(usage.total_tokens) || usageInput + usageOutput
  const cost = numberValue(usage.cost)
  const model = modelName(assignment.model)
  const duration = elapsed(assignment.started_at ?? assignment.created_at, assignment.finished_at)
  const summary = {
    req: items.filter((item) => item.type === 'llm.request').length,
    tools: items.filter((item) => item.type === 'tool.start').length,
    sub: items.filter((item) => item.type === 'run.start' && (itemData(item).parent_run_id || itemData(item).subagent_task)).length,
    steer: items.filter((item) => item.type === 'steer').length,
    in: usageInput,
    out: usageOutput,
    cost: formatCost(cost),
  }

  return <section className="trace-page">
    <header className="trace-toolbar glass-toolbar" data-window-drag-region>
      <button className="trace-icon-button" type="button" aria-label={t('trace.back_to_latest')} onClick={onBack}><ArrowLeft size={16} /></button>
      <div className="trace-heading">
        <div className="trace-breadcrumb"><span>{projectName ?? ''}</span>{projectName && <ChevronRight size={12} />}<span>{botName ?? assignment.bot_id ?? ''}</span><ChevronRight size={12} /><strong>{assignment.title || assignment.instruction || assignment.id}</strong></div>
        <div className="trace-meta"><span className={`trace-status status-${assignment.status ?? 'working'}`}>{assignment.status === 'working' && <LoaderCircle size={12} className="trace-spin" />}{t(statusKey(assignment.status))}</span>{duration && <span>{duration}</span>}{usageTotal > 0 && <span>{formatNumber(usageTotal)} tok</span>}{cost > 0 && <span>¥{formatCost(cost)}</span>}{model && <span>{model}</span>}</div>
      </div>
      <div className="trace-actions">
        <button className={`trace-action-button ${followLatest ? 'is-following' : ''}`} type="button" onClick={() => { setFollowLatest(true); if (scrollRef.current) scrollRef.current.scrollTop = scrollRef.current.scrollHeight }}><span className={`trace-live-dot ${live ? 'is-live' : ''}`} />{t(followLatest ? 'trace.follow_latest' : 'trace.back_to_latest')}</button>
        {stopStatus && onStop && <button className="trace-action-button trace-stop" type="button" onClick={() => onStop(assignment.id)}><Square size={13} />{t('common.stop')}</button>}
        {assignment.bot_id && assignment.origin_chat_id && onMention && <button className="trace-action-button" type="button" onClick={() => onMention(assignment.bot_id!, assignment.origin_chat_id!)}><AtSign size={14} />{t('trace.at_it')}</button>}
        {onScreen && <button className="trace-action-button" type="button" onClick={() => onScreen(assignment.id)}><Monitor size={14} />{t('trace.screen')}</button>}
      </div>
    </header>

    <div className="trace-filterbar">
      <div className="trace-segments" role="tablist" aria-label={t('trace.process')}>
        {(['all', 'model', 'tools', 'subagents', 'steer'] as const).map((key) => <button type="button" role="tab" aria-selected={filter === key} className={filter === key ? 'selected' : ''} key={key} onClick={() => setFilter(key)}>{key === 'all' ? t('trace.process') : t(`trace.filter.${key}` as TraceCopyKey)}</button>)}
      </div>
      <label className="trace-search"><Search size={14} /><input value={search} onChange={(event) => setSearch(event.target.value)} placeholder={t('trace.search')} /></label>
    </div>

    <div className="trace-scroll" ref={scrollRef} onScroll={(event) => {
      const element = event.currentTarget
      const distanceFromBottom = element.scrollHeight - element.scrollTop - element.clientHeight
      if (distanceFromBottom > 40 && followLatest) setFollowLatest(false)
    }}>
      {hasMoreBefore && onLoadMore && <button className="trace-load-more" type="button" disabled={loadingMore} onClick={onLoadMore}>{loadingMore && <LoaderCircle size={13} className="trace-spin" />}{t('trace.state.load_more')}</button>}
      {pendingMessages.length > 0 && <section className="trace-section trace-queue-section">
        <h2><Clock3 size={14} />{t('trace.queue.title')}<span className="trace-count">{pendingMessages.length}</span></h2>
        <p>{t('trace.queue.hint').replace('{bot}', botName ?? assignment.bot_id ?? 'Bot')}</p>
        <div className="trace-queue-list">{pendingMessages.map((message) => <article className={`trace-queue-card ${message.seen ? 'is-seen' : ''}`} key={message.id}>
          <div className="trace-queue-head"><strong>{message.sender}</strong><time>{formatTime(message.at)}</time>{message.seen && <span className="trace-seen"><Check size={12} />{t('trace.queue.seen')}</span>}</div>
          <div className="trace-queue-text">{message.text}</div>
          <div className="trace-queue-actions">{!message.seen && message.steerable !== false && onQueueSteer && <button type="button" onClick={() => onQueueSteer(message.id)}><AtSign size={12} />{t('trace.queue.to_steer')}</button>}</div>
        </article>)}</div>
      </section>}

      <section className="trace-section trace-instruction-section">
        <h2><FileText size={14} />{t('trace.instruction')}</h2>
        <div className="trace-instruction-card">{assignment.instruction?.trim() || t('trace.state.instruction_empty')}</div>
      </section>

      <section className="trace-section trace-process-section">
        <h2><Activity size={14} />{t('trace.process')}</h2>
        {loading && items.length === 0 && <div className="trace-state" aria-busy="true"><LoaderCircle size={16} className="trace-spin" />{t('trace.state.loading')}</div>}
        {error && <div className="trace-error" role="alert"><AlertCircle size={15} /><span>{t('trace.state.error')} · {error}</span>{onRetry && <button type="button" onClick={onRetry}>{t('common.retry')}</button>}</div>}
        {!loading && !error && visibleEntries.length === 0 && inFlight.length === 0 && <div className={`trace-state ${search || filter !== 'all' ? '' : 'trace-empty'}`} role="status">
          <span>{search || filter !== 'all' ? t('trace.state.no_matches') : t('trace.state.empty')}</span>
          {!search && filter === 'all' && onRetry && <button type="button" onClick={onRetry}>{t('common.retry')}</button>}
        </div>}
        {(visibleEntries.length > 0 || inFlight.length > 0) && <div className="trace-timeline">
          {visibleEntries.map((entry) => <TraceRow key={`${entry.item.run_id}-${entry.item.aseq}`} entry={entry} t={t} onOpenThinking={onOpenThinking} queueReviewDetails={queueReviewDetails} toolOutputs={toolOutputs} />)}
          {inFlight.map((request) => <article className="trace-row trace-row-live" key={`flight-${request.request_id}`}>
            <time className="trace-time">{t('trace.running')}</time><span className="trace-row-icon model"><LoaderCircle size={14} className="trace-spin" /></span><div className="trace-row-body"><strong>{t('trace.running')}</strong>{request.thinking && <div className="trace-inline-thinking"><Brain size={12} />{request.thinking}</div>}{request.text && <pre>{request.text}</pre>}{request.tool_args && <pre>{request.tool_args}</pre>}</div><span className="trace-result">{t('trace.running')}</span>
          </article>)}
        </div>}
      </section>
    </div>

    <footer className="trace-summary"><span>{t('trace.summary').replace('{req}', String(summary.req)).replace('{tools}', String(summary.tools)).replace('{sub}', String(summary.sub)).replace('{steer}', String(summary.steer)).replace('{in}', formatNumber(summary.in)).replace('{out}', formatNumber(summary.out)).replace('{cost}', `¥${summary.cost}`)}</span><div>{onOpenThinking && latestThinkingItem && <button type="button" onClick={() => onOpenThinking(latestThinkingItem)}><Brain size={13} />{t('trace.thinking_summary')}<ChevronRight size={13} /></button>}{onOpenFullLog && <button type="button" onClick={() => onOpenFullLog(assignment.id)}><FileText size={13} />{t('trace.full_log')}<ChevronRight size={13} /></button>}</div></footer>
  </section>
}

function TraceRow({ entry, t, onOpenThinking, queueReviewDetails, toolOutputs }: { entry: TimelineEntry; t: (key: TraceCopyKey) => string; onOpenThinking?: (item: TraceItem) => void; queueReviewDetails: NonNullable<TracePageProps['queueReviewDetails']>; toolOutputs: Record<string, string> }) {
  const [expanded, setExpanded] = useState(false)
  const data = itemData(entry.item)
  const thinking = stringValue(data.thinking)
  const args = objectValue(data.args)
  const details = objectValue(data.details)
  const callId = stringValue(data.call_id)
  const toolOutput = toolOutputs[callId] || stringValue(data.preview)
  const errorText = stringValue(data.error)
  const result = entry.failed ? t('trace.state.tool_failed') : entry.result

  return <article className={`trace-row ${entry.failed ? 'is-error' : ''} ${entry.subagent ? 'is-subagent' : ''}`}>
    <time className="trace-time">{formatTime(entry.item.at)}</time>
    <span className={`trace-row-icon ${entry.kind}`}><entry.icon size={14} /></span>
    <div className="trace-row-body">
      <div className="trace-row-title"><strong>{entry.title}</strong>{entry.detail && <span>{entry.detail}</span>}{entry.item.type === 'llm.request' && Array.isArray(data.tools) && <span>{(data.tools as unknown[]).length} tools</span>}</div>
      {entry.item.type === 'llm.request' && <div className="trace-context"><span>{t('trace.state.request_context')}: {formatNumber(numberValue(objectValue(data.context)?.total))}</span>{stringValue(objectValue(data.model)?.model_id) && <span>{stringValue(objectValue(data.model)?.model_id)}</span>}</div>}
      {entry.item.type === 'llm.response' && (stringValue(data.text) || thinking) && <div className="trace-response">
        {stringValue(data.text) && <p>{stringValue(data.text)}</p>}
        {thinking && <details className="trace-thinking"><summary><Brain size={12} />{t('trace.state.thinking')}</summary><p>{thinking}</p></details>}
        {onOpenThinking && thinking && <button className="trace-inline-action" type="button" onClick={() => onOpenThinking(entry.item)}>{t('trace.thinking_summary')}<ChevronRight size={12} /></button>}
      </div>}
      {entry.item.type === 'tool.start' && args && <pre className="trace-code">{JSON.stringify(args, null, 2)}</pre>}
      {entry.item.type === 'tool.start' && toolOutputs[callId] && <pre className="trace-code">{toolOutputs[callId]}</pre>}
      {entry.item.type === 'tool.end' && toolOutput && <pre className="trace-code">{toolOutput}</pre>}
      {entry.item.type === 'run.start' && stringValue(data.subagent_task) && <div className="trace-subagent-task"><Layers3 size={12} />{t('trace.state.subagent_task')}: {stringValue(data.subagent_task)}</div>}
      {entry.item.type === 'steer' && stringValue(data.text) && <div className="trace-steer-text">{stringValue(data.text)} <span>→ {t('trace.item.steer_applied')}</span></div>}
      {entry.item.type === 'queue.review' && Array.isArray(data.message_ids) && <details className="trace-details"><summary>{t('trace.queue.review_details')}</summary><div className="trace-reviewed-list">{(data.message_ids as unknown[]).map((id) => { const row = queueReviewDetails[String(id)]; return <div className="trace-reviewed-item" key={String(id)}><span>{t('trace.queue.review_message')}: {row?.text ?? String(id)}</span>{row?.result && <span>→ {t('trace.queue.review_result')}: {row.result}</span>}</div> })}</div></details>}
      {entry.item.type === 'tool.end' && details && <details className="trace-details"><summary>{t('trace.full_log')}</summary><pre>{JSON.stringify(details, null, 2)}</pre></details>}
      {entry.item.type === 'tool.end' && errorText && <div className="trace-error-line"><CircleAlert size={12} />{errorText}</div>}
    </div>
    <span className={`trace-result ${entry.failed ? 'failed' : ''}`}>{result}</span>
    {entry.item.type === 'llm.response' && thinking && <button className="trace-row-open" type="button" aria-label={t('trace.thinking_summary')} onClick={() => setExpanded((value) => !value)}><ChevronRight size={13} className={expanded ? 'rotated' : ''} /></button>}
    {expanded && <div className="trace-expanded">{onOpenThinking ? <button type="button" onClick={() => onOpenThinking(entry.item)}>{t('trace.thinking_summary')}<ChevronRight size={12} /></button> : thinking}</div>}
  </article>
}

function toTimelineEntry(item: TraceItem, t: (key: TraceCopyKey) => string, requestNumber: number, relatedModel?: string, relatedTool?: string): TimelineEntry {
  const data = itemData(item)
  const model = modelName(objectValue(data.model) as TraceAssignment['model'])
  const tool = stringValue(data.name)
  const type = item.type
  if (type === 'run.start') {
    const subagent = Boolean(data.parent_run_id || data.subagent_task)
    return { item, kind: subagent ? 'subagents' : 'all', title: subagent ? t('trace.item.subagents').replace('{count}', '1') : t('trace.item.start'), detail: subagent ? stringValue(data.subagent_task) : model, icon: subagent ? Layers3 : Activity, subagent }
  }
  if (type === 'llm.request') return { item, kind: 'model', title: t('trace.item.model_request').replace('{n}', String(requestNumber)), detail: model, result: `${formatNumber(numberValue(objectValue(data.context)?.total))} tok`, icon: Brain }
  if (type === 'llm.response') return { item, kind: 'model', title: t('trace.item.model_request').replace('{n}', String(requestNumber)), detail: relatedModel ?? model, result: formatDurationMs(numberValue(data.latency_ms)), icon: Brain, failed: data.stop_reason === 'error' }
  if (type === 'tool.start') return { item, kind: 'tools', title: tool || 'tool', detail: compactJson(data.args), icon: toolIcon(tool) }
  if (type === 'tool.end') { const name = relatedTool ?? tool; return { item, kind: 'tools', title: name || 'tool', detail: '', result: formatDurationMs(numberValue(data.duration_ms)), icon: toolIcon(name), failed: data.is_error === true } }
  if (type === 'steer') return { item, kind: 'steer', title: t('trace.item.steer').replace('{who}', senderLabel(data.from, t)), detail: stringValue(data.text), icon: AtSign }
  if (type === 'queue.review') return { item, kind: 'steer', title: t('trace.item.queue_review').replace('{count}', String(numberValue(data.count))), detail: '', result: '', icon: Clock3 }
  if (type === 'send_msg') return { item, kind: 'tools', title: stringValue(objectValue(data.intent)?.kind) || type, detail: stringValue(data.message_id), icon: Activity }
  if (type === 'run.wait') return { item, kind: 'all', title: t('trace.state.waiting'), detail: String(data.reason ?? ''), icon: Clock3 }
  if (type === 'run.resume') return { item, kind: 'all', title: t('trace.state.resumed'), detail: '', icon: Activity }
  if (type === 'compaction') return { item, kind: 'model', title: t('trace.state.compaction'), detail: stringValue(data.reason), result: `${formatNumber(numberValue(data.before_tokens))} → ${formatNumber(numberValue(data.after_tokens))}`, icon: Layers3 }
  if (type === 'run.end') {
    const endStatus = String(objectValue(data.status)?.kind ?? data.status ?? '')
    const title = endStatus === 'failed' ? t('trace.state.run_failed')
      : endStatus === 'cancelled' ? t('status.cancelled')
        : endStatus === 'suspended' ? t('trace.state.waiting')
          : endStatus === 'done' ? t('trace.state.returned') : t('trace.state.unknown')
    const failed = endStatus === 'failed' || Boolean(data.error)
    return { item, kind: 'all', title, detail: stringValue(data.error), icon: failed || endStatus === 'cancelled' ? XCircle : endStatus === 'suspended' ? Clock3 : Check, failed }
  }
  return { item, kind: 'all', title: t('trace.state.unknown'), detail: type, result: compactJson(data), icon: Activity }
}

function statusKey(status?: string): TraceCopyKey {
  const normalized = (status ?? 'working').toLowerCase().replaceAll('-', '_')
  if (normalized === 'waiting_bot' || normalized === 'suspended') return 'trace.state.waiting'
  return (['queued', 'working', 'waiting_user', 'blocked', 'done', 'failed', 'cancelled'].includes(normalized) ? `status.${normalized}` : 'trace.state.unknown') as TraceCopyKey
}

function itemData(item: TraceItem): Record<string, unknown> { return item.data && typeof item.data === 'object' ? item.data : {} }
function objectValue(value: unknown): Record<string, unknown> | undefined { return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : undefined }
function stringValue(value: unknown): string { return typeof value === 'string' ? value : '' }
function numberValue(value: unknown): number { return typeof value === 'number' && Number.isFinite(value) ? value : 0 }
function modelName(value?: TraceAssignment['model']): string { return [value?.provider, value?.model_id].filter((part): part is string => typeof part === 'string' && part.length > 0).join(' · ') }
function senderLabel(value: unknown, t: (key: TraceCopyKey) => string): string { const sender = objectValue(value); return sender?.kind === 'user' ? t('trace.state.from') : stringValue(sender?.bot_id) || t('trace.state.from') }
function compactJson(value: unknown): string { if (!value || typeof value !== 'object') return ''; const text = JSON.stringify(value); return text.length > 180 ? `${text.slice(0, 177)}…` : text }
function formatNumber(value: number): string { return new Intl.NumberFormat().format(value) }
function formatCost(value: number): string { return value.toFixed(2) }
function formatTime(value?: string): string { if (!value) return ''; const date = new Date(value); return Number.isNaN(date.getTime()) ? '' : date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' }) }
function formatDurationMs(value: number): string { return value > 0 ? `${(value / 1000).toFixed(1)}s` : '' }
function elapsed(start?: string | null, finish?: string | null): string { if (!start) return ''; const begin = new Date(start).getTime(); const end = finish ? new Date(finish).getTime() : Date.now(); if (!Number.isFinite(begin) || !Number.isFinite(end)) return ''; const minutes = Math.max(0, Math.floor((end - begin) / 60_000)); return minutes < 60 ? `${minutes}m` : `${Math.floor(minutes / 60)}h ${minutes % 60}m` }
function toolIcon(name: string) { return /bash|terminal|shell/i.test(name) ? Terminal : /read|write|file|edit/i.test(name) ? FileText : Wrench }
