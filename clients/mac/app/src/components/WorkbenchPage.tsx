import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import { AlertTriangle, AtSign, Check, Clock3, MessageCircle, Shield, Square, SquareArrowOutUpRight, Workflow } from 'lucide-react'
import { invoke } from '@tauri-apps/api/core'
import { t } from '../i18n'
import type { BridgeState, Json } from '../types'

type Assignment = Record<string, Json>
type Workbench = {
  running: number
  global_limit: number
  subagents_running: number
  waiting: Record<string, Json>[]
  bots: { bot_id: string; active: number; max_parallel: number; assignments: Assignment[] }[]
  done_today: Assignment[]
}

export function WorkbenchPage({ state, onOpenChat, onTrace, onMention, onRefresh }: { state: BridgeState; onOpenChat: (chatId: string) => void; onTrace: (assignmentId: string, chatId?: string) => void; onMention: (botId: string, chatId: string) => void; onRefresh: () => void }) {
  const [data, setData] = useState<Workbench | null>(null)
  const [view, setView] = useState<'by_bot' | 'by_group' | 'by_status'>('by_bot')
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    if (state.connection !== 'connected') return
    setLoading(true)
    setError(null)
    try {
      setData(await invoke<Workbench>('protocol_request', { method: 'workbench.get', params: {} }))
    } catch (reason) {
      setError(String(reason))
    } finally {
      setLoading(false)
    }
  }, [state.connection])

  useEffect(() => {
    void load()
    const interval = window.setInterval(() => void load(), 15_000)
    return () => window.clearInterval(interval)
  }, [load])

  const assignments = useMemo(() => data?.bots.flatMap((bot) => bot.assignments) ?? [], [data])
  const groups = useMemo(() => {
    const grouped = new Map<string, Assignment[]>()
    for (const assignment of assignments) {
      const key = stringValue(assignment.project_id) || stringValue(assignment.origin_chat_id) || 'unassigned'
      grouped.set(key, [...(grouped.get(key) ?? []), assignment])
    }
    return [...grouped.entries()]
  }, [assignments])
  const statuses = useMemo(() => {
    const grouped = new Map<string, Assignment[]>()
    for (const assignment of assignments) {
      const key = stringValue(assignment.status) || 'working'
      grouped.set(key, [...(grouped.get(key) ?? []), assignment])
    }
    return [...grouped.entries()]
  }, [assignments])

  const waitingReview = (data?.waiting ?? []).filter((item) => item.kind === 'review')
  const waitingApproval = (data?.waiting ?? []).filter((item) => item.kind === 'approval')
  const done = data?.done_today ?? []

  return <section className="desktop-page workbench-page">
    <header className="desktop-page-toolbar glass-toolbar" data-window-drag-region><div className="desktop-page-title"><Workflow size={17} />{t('workbench.title')}</div><div className="workbench-toolbar-controls">
      <div className="segmented-control" role="tablist" aria-label={t('workbench.title')}>
        <button className={view === 'by_bot' ? 'selected' : ''} onClick={() => setView('by_bot')}>{t('workbench.view.by_bot')}</button>
        <button className={view === 'by_group' ? 'selected' : ''} onClick={() => setView('by_group')}>{t('workbench.view.by_group')}</button>
        <button className={view === 'by_status' ? 'selected' : ''} onClick={() => setView('by_status')}>{t('workbench.view.by_status')}</button>
      </div><span className="workbench-global-status">{t('workbench.running').replace('{running}', String(data?.running ?? 0)).replace('{limit}', String(data?.global_limit ?? 0))}</span>
    </div></header>
    <div className="desktop-page-scroll workbench-scroll">
      {(waitingReview.length > 0 || waitingApproval.length > 0) && <div className="workbench-waiting-banners">
        {waitingReview.map((item, index) => <button className="workbench-waiting-banner review" key={`review-${index}`} onClick={() => {
          const projectId = stringValue(item.project_id)
          const chat = Object.values(state.chats).find((entry) => entry.project_id === projectId)
          if (chat) onOpenChat(chat.id)
        }}><Check size={15} /><strong>{t('workbench.waiting_review')}</strong><span>{state.projects[stringValue(item.project_id)]?.name ?? ''}</span><SquareArrowOutUpRight size={13} /></button>)}
        {waitingApproval.map((item, index) => {
          const approval = item.approval as Record<string, Json> | undefined
          return <button className="workbench-waiting-banner approval" key={`approval-${index}`} onClick={() => {
            const chatId = stringValue(approval?.chat_id)
            if (chatId) onOpenChat(chatId)
          }}><Shield size={15} /><strong>{t('workbench.waiting_approval')}</strong><span>{String(approval?.summary ?? approval?.tool ?? '')}</span><SquareArrowOutUpRight size={13} /></button>
        })}
      </div>}
      {error && <div className="workbench-error" role="alert"><AlertTriangle size={14} />{error}<button onClick={() => void load()}>{t('common.retry')}</button></div>}
      {loading && !data ? <div className="desktop-page-empty" aria-busy="true" /> : view === 'by_bot'
        ? data?.bots.map((bot) => <BotTaskCard key={bot.bot_id} bot={bot} state={state} onOpenChat={onOpenChat} onTrace={onTrace} onMention={onMention} onRefresh={() => { void load(); onRefresh() }} />)
        : view === 'by_group'
          ? groups.map(([key, items]) => <TaskGroup key={key} title={state.projects[key]?.name ?? state.chats[key]?.title ?? key} assignments={items} state={state} onOpenChat={onOpenChat} onTrace={onTrace} onMention={onMention} onRefresh={() => { void load(); onRefresh() }} />)
          : statuses.map(([status, items]) => <TaskGroup key={status} title={statusText(status)} assignments={items} state={state} onOpenChat={onOpenChat} onTrace={onTrace} onMention={onMention} onRefresh={() => { void load(); onRefresh() }} />)}
      {!loading && assignments.length === 0 && !error && <div className="desktop-page-empty">{t('follow.empty')}</div>}
      {data?.bots.some((bot) => state.bots[bot.bot_id]?.is_main) && <div className="workbench-main-note">{t('workbench.main_not_counted')}</div>}
      <div className="workbench-done-heading">{t('workbench.done_today').replace('{count}', String(done.length))}</div>
      <DoneList rows={done} state={state} onTrace={onTrace} />
    </div>
  </section>
}

function BotTaskCard({ bot, state, onOpenChat, onTrace, onMention, onRefresh }: { bot: Workbench['bots'][number]; state: BridgeState; onOpenChat: (id: string) => void; onTrace: (id: string, chatId?: string) => void; onMention: (botId: string, chatId: string) => void; onRefresh: () => void }) {
  const botInfo = state.bots[bot.bot_id]
  return <section className="workbench-bot-card">
    <header><span className="workbench-bot-avatar">{String(botInfo?.name ?? '?').slice(0, 1)}</span><strong>{botInfo?.name ?? bot.bot_id}</strong><span className="workbench-concurrency">{t('workbench.concurrency').replace('{running}', String(bot.active)).replace('{max}', String(bot.max_parallel))}</span></header>
    <AssignmentList rows={bot.assignments} state={state} onOpenChat={onOpenChat} onTrace={onTrace} onMention={onMention} onRefresh={onRefresh} />
  </section>
}

function TaskGroup({ title, assignments, state, onOpenChat, onTrace, onMention, onRefresh }: { title: string; assignments: Assignment[]; state: BridgeState; onOpenChat: (id: string) => void; onTrace: (id: string, chatId?: string) => void; onMention: (botId: string, chatId: string) => void; onRefresh: () => void }) {
  return <section className="workbench-bot-card workbench-group-card"><header><strong>{title}</strong><span className="workbench-concurrency">{assignments.length}</span></header><AssignmentList rows={assignments} state={state} onOpenChat={onOpenChat} onTrace={onTrace} onMention={onMention} onRefresh={onRefresh} /></section>
}

function AssignmentList({ rows, ...props }: { rows: Assignment[]; state: BridgeState; onOpenChat: (id: string) => void; onTrace: (id: string, chatId?: string) => void; onMention: (botId: string, chatId: string) => void; onRefresh: () => void }) {
  const parentRef = useRef<HTMLDivElement>(null)
  const virtualizer = useVirtualizer({ count: rows.length, getScrollElement: () => parentRef.current, estimateSize: () => 48, overscan: 6, useFlushSync: false })
  return <div className="workbench-assignment-list" ref={parentRef} style={{ maxHeight: rows.length > 10 ? 480 : undefined }}>
    <div className="workbench-assignment-inner" style={{ height: `${virtualizer.getTotalSize()}px` }}>
      {virtualizer.getVirtualItems().map((item) => <div key={item.key} ref={virtualizer.measureElement} data-index={item.index} className="workbench-assignment-virtual-row" style={{ transform: `translateY(${item.start}px)` }}>
        <AssignmentRow assignment={rows[item.index]} state={props.state} onOpenChat={props.onOpenChat} onTrace={props.onTrace} onMention={props.onMention} onRefresh={props.onRefresh} />
      </div>)}
    </div>
  </div>
}

function AssignmentRow({ assignment, state, onOpenChat, onTrace, onMention, onRefresh }: { assignment: Assignment; state: BridgeState; onOpenChat: (id: string) => void; onTrace: (id: string, chatId?: string) => void; onMention: (botId: string, chatId: string) => void; onRefresh: () => void }) {
  const id = stringValue(assignment.id)
  const chatId = stringValue(assignment.project_id)
    ? Object.values(state.chats).find((chat) => chat.project_id === assignment.project_id)?.id ?? ''
    : stringValue(assignment.origin_chat_id)
  const status = stringValue(assignment.status)
  const usage = assignment.usage as Record<string, Json> | undefined
  const model = assignment.model as Record<string, Json> | undefined
  const tokens = Number(usage?.input_tokens ?? 0) + Number(usage?.output_tokens ?? 0)
  const duration = elapsed(assignment.started_at, assignment.finished_at)
  return <div className="workbench-task-row">
    <span className={`workbench-task-state ${status}`}>{status === 'working' ? <span className="spinner" /> : status === 'queued' ? <Clock3 size={13} /> : <AlertTriangle size={13} />}</span>
    <button className="workbench-task-name" onClick={() => onTrace(id, chatId)}><strong>{stringValue(assignment.title) || stringValue(assignment.instruction)}</strong>{Number(assignment.pending_messages ?? 0) > 0 && <span className="workbench-queued-pill"><Clock3 size={11} />{t('workbench.queued_msgs').replace('{count}', String(assignment.pending_messages))}</span>}<small>{statusText(status)}{duration ? ` · ${duration}` : ''}</small></button>
    <span className="workbench-task-data">{tokens > 0 ? shortNumber(tokens) : ''}</span><span className="workbench-task-model">{String(model?.model_id ?? model?.model ?? '')}</span><span className="workbench-task-data">{Number(assignment.subagents_active ?? 0) || ''}</span>
    <button className="workbench-action secondary" disabled={!chatId} onClick={() => chatId && onOpenChat(chatId)}><MessageCircle size={13} />{t('follow.open_group')}</button>
    <button className="workbench-action" disabled={!chatId || !stringValue(assignment.bot_id)} onClick={() => chatId && onMention(stringValue(assignment.bot_id), chatId)}><AtSign size={13} />{t('trace.at_it')}</button>
    <button className="workbench-action" onClick={() => onTrace(id, chatId)}>{t('common.details')}</button>
    <button className="workbench-action stop" disabled={status !== 'working' && status !== 'waiting_user' && status !== 'waiting_bot'} onClick={async () => {
      await invoke('protocol_request', { method: 'assignment.stop', params: { assignment_id: id, client_request_id: crypto.randomUUID() } })
      onRefresh()
    }}><Square size={13} />{t('common.stop')}</button>
  </div>
}

function DoneList({ rows, state, onTrace }: { rows: Assignment[]; state: BridgeState; onTrace: (id: string, chatId?: string) => void }) {
  const parentRef = useRef<HTMLDivElement>(null)
  const virtualizer = useVirtualizer({ count: rows.length, getScrollElement: () => parentRef.current, estimateSize: () => 42, overscan: 6, useFlushSync: false })
  return <div className="workbench-done-list" ref={parentRef}>
    <div className="workbench-done-inner" style={{ height: `${virtualizer.getTotalSize()}px` }}>
      {virtualizer.getVirtualItems().map((item) => {
        const row = rows[item.index]
        const projectId = stringValue(row.project_id)
        const chatId = projectId ? Object.values(state.chats).find((chat) => chat.project_id === projectId)?.id ?? '' : stringValue(row.origin_chat_id)
        return <button className="workbench-done-row" key={item.key} ref={virtualizer.measureElement} data-index={item.index} style={{ transform: `translateY(${item.start}px)` }} onClick={() => onTrace(stringValue(row.id), chatId)}>
          <Check size={14} /><strong>{projectId ? state.projects[projectId]?.name ?? '' : state.chats[chatId]?.title ?? ''} · {stringValue(row.title)}</strong><span>{state.bots[stringValue(row.bot_id)]?.name ?? ''}</span><small>{elapsed(row.started_at, row.finished_at)}</small>
        </button>
      })}
    </div>
  </div>
}

function statusText(status: string) {
  const key: Record<string, Parameters<typeof t>[0]> = { working: 'status.working', queued: 'status.queued', waiting_user: 'status.waiting_user', waiting_bot: 'status.waiting_user', blocked: 'status.blocked', done: 'status.done', failed: 'status.failed', cancelled: 'status.cancelled' }
  return t(key[status] ?? 'status.working')
}
function stringValue(value: Json | undefined): string { return typeof value === 'string' ? value : '' }
function elapsed(startValue: Json | undefined, endValue: Json | undefined) {
  if (typeof startValue !== 'string') return ''
  const start = Date.parse(startValue)
  if (!Number.isFinite(start)) return ''
  const seconds = Math.max(0, Math.floor(((typeof endValue === 'string' ? Date.parse(endValue) : Date.now()) - start) / 1000))
  return seconds < 3600
    ? t('common.minutes').replace('{count}', String(Math.floor(seconds / 60)))
    : t('common.hours_minutes').replace('{h}', String(Math.floor(seconds / 3600))).replace('{m}', String(Math.floor((seconds % 3600) / 60)))
}
function shortNumber(value: number) { return value >= 1_000_000 ? `${(value / 1_000_000).toFixed(1)}M` : value >= 10_000 ? `${(value / 1_000).toFixed(1)}k` : value.toLocaleString('zh-CN') }
