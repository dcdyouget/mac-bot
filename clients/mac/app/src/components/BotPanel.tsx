import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type ReactNode } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { AlertCircle, ArrowUpRight, ChevronDown, ChevronLeft, ChevronRight, Ellipsis, LoaderCircle, Monitor, Settings2, Sparkles, Trash2 } from 'lucide-react'
import type { Json } from '../types'
import './BotPanel.css'

export type BotRecord = {
  id: string
  name: string
  label?: string
  description?: string
  avatar?: { kind?: string; color?: number; emoji?: string; file?: unknown } | null
  is_main?: boolean
  model?: string | null
  max_parallel?: number
  tools?: Record<string, boolean>
  browser_mode?: 'headless' | 'headless_profile' | 'attach'
  pinned?: boolean
  hidden?: boolean
  notifications?: boolean
  dm_chat_id?: string
  status?: { summary?: string; active?: number; queued?: number; waiting?: number }
  [key: string]: unknown
}

export type BotProject = {
  id: string
  name: string
  chat_id?: string
  status?: string
  members?: { bot_id?: string; role_note?: string }[]
}

export type BotAssignment = {
  id: string
  bot_id: string
  project_id?: string | null
  origin_chat_id?: string
  title?: string
  instruction?: string
  status?: string
  created_at?: string
  started_at?: string | null
  finished_at?: string | null
  usage?: { input_tokens?: number; output_tokens?: number; cost?: number | null }
  model?: string
}

type ModelOption = { ref: string; display_name?: string; enabled?: boolean; provider_id?: string }
type SettingsRecord = { timezone?: string; models?: { bot_default?: string | null; main?: string | null }; main_bot?: { auto_create_project?: boolean } }
type UsageTotals = { input_tokens?: number; output_tokens?: number; cost?: number | null }
type HistoryResponse = { items?: BotAssignment[]; next_cursor?: string | null }
type UsageResponse = { rows?: { key: string; usage?: UsageTotals }[] }
type SkillsResponse = { skills?: { enabled: boolean; disabled_bot_ids?: string[] }[] }
type BotView = 'detail' | 'settings'

type Props = {
  botId: string
  bot: BotRecord
  projects: BotProject[]
  chats: Record<string, { id: string; title?: string; project_id?: string | null }>
  assignments: Record<string, Record<string, Json>>
  hostScope: string
  onClose: () => void
  onOpenChat: (chatId: string) => void
  onOpenTrace: (assignmentId: string) => void
  onOpenComputer: (botId: string) => void
  onOpenSkills?: (botId: string) => void
  onBotUpdated: (bot: BotRecord) => void
  onBotDeleted: (botId: string) => void
  onBotDuplicated: (bot: BotRecord, dmChatId: string) => void
  t: (key: string) => string
}

type Draft = {
  name: string
  label: string
  description: string
  avatarKind: 'bean' | 'emoji'
  avatarColor: number
  avatarEmoji: string
  model: string
  maxParallel: number
  tools: Record<string, boolean>
  browserMode: 'headless' | 'headless_profile' | 'attach'
  notifications: boolean
  autoCreateProject: boolean
}

const TOOL_KEYS = ['files', 'bash', 'browser', 'subagent', 'web', 'mcp'] as const
const TERMINAL_STATUSES = ['done', 'failed', 'cancelled']
const AVATAR_COLORS = Array.from({ length: 10 }, (_, index) => index)

function usageWindow(timezone: string) {
  const now = new Date()
  const formatter = new Intl.DateTimeFormat('en-CA', { timeZone: timezone || 'UTC', year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', second: '2-digit', hourCycle: 'h23' })
  const parts = Object.fromEntries(formatter.formatToParts(now).map((part) => [part.type, part.value]))
  const localDate = `${parts.year}-${parts.month}-${parts.day}`
  const date = new Date(`${localDate}T00:00:00Z`)
  date.setUTCDate(date.getUTCDate() - ((date.getUTCDay() + 6) % 7))
  const monday = date.toISOString().slice(0, 10)
  const targetWallTime = Date.parse(`${monday}T00:00:00Z`)
  let instant = targetWallTime
  for (let attempt = 0; attempt < 3; attempt += 1) {
    const local = Object.fromEntries(formatter.formatToParts(new Date(instant)).map((part) => [part.type, part.value]))
    const representedWallTime = Date.UTC(Number(local.year), Number(local.month) - 1, Number(local.day), Number(local.hour), Number(local.minute), Number(local.second))
    const adjustment = targetWallTime - representedWallTime
    if (!adjustment) break
    instant += adjustment
  }
  return { from: new Date(instant).toISOString(), to: now.toISOString() }
}

function makeDraft(bot: BotRecord, settings?: SettingsRecord): Draft {
  const avatar = bot.avatar ?? { kind: 'bean', color: 0 }
  return {
    name: bot.name,
    label: bot.label ?? '',
    description: bot.description ?? '',
    avatarKind: avatar.kind === 'emoji' ? 'emoji' : 'bean',
    avatarColor: Number.isInteger(avatar.color) ? Number(avatar.color) : 0,
    avatarEmoji: avatar.emoji ?? '',
    model: bot.model ?? (bot.is_main ? settings?.models?.main : settings?.models?.bot_default) ?? '',
    maxParallel: Math.max(1, Math.min(8, Number(bot.max_parallel ?? 3))),
    tools: { ...(bot.tools ?? {}) },
    browserMode: bot.browser_mode ?? 'headless',
    notifications: bot.notifications ?? true,
    autoCreateProject: settings?.main_bot?.auto_create_project ?? true,
  }
}

function errorText(error: unknown) { return error instanceof Error ? error.message : String(error) }
function formatNumber(value: number) { return new Intl.NumberFormat('zh-CN', { notation: value >= 10_000 ? 'compact' : 'standard', maximumFractionDigits: 1 }).format(value) }
function formatCost(value: number | null | undefined) { return value == null ? '—' : `¥${value.toFixed(2)}` }
function shortDate(value?: string | null) {
  if (!value) return '—'
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? '—' : date.toLocaleDateString('zh-CN', { month: '2-digit', day: '2-digit' })
}
function elapsed(assignment: BotAssignment, t: Props['t']) {
  const start = assignment.started_at ?? assignment.created_at
  if (!start) return '—'
  const end = assignment.finished_at ? new Date(assignment.finished_at).getTime() : Date.now()
  const minutes = Math.max(0, Math.round((end - new Date(start).getTime()) / 60_000))
  return minutes >= 60
    ? t('common.hours_minutes').replace('{h}', String(Math.floor(minutes / 60))).replace('{m}', String(minutes % 60))
    : t('common.minutes').replace('{count}', String(minutes))
}

export function BotPanel(props: Props) {
  const { botId, bot: initialBot, projects, chats, assignments: liveAssignments, hostScope, onClose, onOpenChat, onOpenTrace, onOpenComputer, onOpenSkills, onBotUpdated, onBotDeleted, onBotDuplicated, t } = props
  const [view, setView] = useState<BotView>('detail')
  const [bot, setBot] = useState(initialBot)
  const [settings, setSettings] = useState<SettingsRecord | null>(null)
  const [draft, setDraft] = useState(() => makeDraft(initialBot))
  const [history, setHistory] = useState<BotAssignment[]>([])
  const [cursor, setCursor] = useState<string | null>(null)
  const [historyLoading, setHistoryLoading] = useState(false)
  const [historyError, setHistoryError] = useState<string | null>(null)
  const [projectFilter, setProjectFilter] = useState('')
  const [weekUsage, setWeekUsage] = useState<UsageTotals | null>(null)
  const [usageError, setUsageError] = useState<string | null>(null)
  const [models, setModels] = useState<ModelOption[]>([])
  const [skillCount, setSkillCount] = useState<number | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [saveError, setSaveError] = useState<string | null>(null)
  const [duplicateOpen, setDuplicateOpen] = useState(false)
  const [duplicateName, setDuplicateName] = useState('')
  const [confirmDelete, setConfirmDelete] = useState(false)
  const [toast, setToast] = useState('')
  const initialBotRef = useRef(initialBot)
  const onBotUpdatedRef = useRef(onBotUpdated)
  initialBotRef.current = initialBot
  onBotUpdatedRef.current = onBotUpdated

  const chatForProject = useCallback((project: BotProject) => project.chat_id ?? Object.values(chats).find((chat) => chat.project_id === project.id)?.id, [chats])
  const memberProjects = useMemo(() => projects.filter((project) => project.members?.some((member) => member.bot_id === botId)), [projects, botId])
  const currentAssignments = useMemo(() => Object.values(liveAssignments).filter((assignment): assignment is BotAssignment => typeof assignment.id === 'string' && assignment.bot_id === botId && ['working', 'waiting_user', 'waiting_bot', 'blocked', 'queued'].includes(String(assignment.status))), [liveAssignments, botId])
  const visibleHistory = useMemo(() => history.filter((assignment) => !projectFilter || assignment.project_id === projectFilter), [history, projectFilter])

  useEffect(() => {
    let active = true
    setView('detail')
    setBot(initialBotRef.current)
    setDraft(makeDraft(initialBotRef.current))
    setHistory([])
    setCursor(null)
    setHistoryError(null)
    setUsageError(null)
    setLoadError(null)
    setWeekUsage(null)
    setSettings(null)
    setHistoryLoading(true)
    const scope = hostScope
    void Promise.allSettled([
      invoke<{ bot: BotRecord }>('protocol_request', { method: 'bot.get', params: { bot_id: botId } }),
      invoke<{ settings: SettingsRecord }>('protocol_request', { method: 'settings.get', params: {} }),
      invoke<HistoryResponse>('protocol_request', { method: 'assignment.list', params: { bot_id: botId, status: TERMINAL_STATUSES, limit: 40 } }),
    ]).then(async ([botResult, settingsResult, historyResult]) => {
      if (!active) return
      if (botResult.status === 'fulfilled') {
        setBot(botResult.value.bot)
        onBotUpdatedRef.current(botResult.value.bot)
      } else setLoadError(errorText(botResult.reason))
      const loadedSettings = settingsResult.status === 'fulfilled' ? settingsResult.value.settings : null
      setSettings(loadedSettings)
      const currentBot = botResult.status === 'fulfilled' ? botResult.value.bot : initialBotRef.current
      setDraft(makeDraft(currentBot, loadedSettings ?? undefined))
      if (historyResult.status === 'fulfilled') {
        setHistory(historyResult.value.items ?? [])
        setCursor(historyResult.value.next_cursor ?? null)
      } else setHistoryError(errorText(historyResult.reason))
      setHistoryLoading(false)
      if (settingsResult.status === 'rejected') setLoadError((existing) => existing ?? errorText(settingsResult.reason))
      const range = usageWindow(loadedSettings?.timezone ?? 'UTC')
      try {
        const usage = await invoke<UsageResponse>('protocol_request', { method: 'usage.breakdown', params: { ...range, dimension: 'bot', drill: { bot_id: botId } } })
        if (active && hostScope === scope) setWeekUsage(usage.rows?.find((row) => row.key === botId)?.usage ?? null)
      } catch (error) {
        if (active && hostScope === scope) setUsageError(errorText(error))
      }
    }).catch((error: unknown) => { if (active) { setLoadError(errorText(error)); setHistoryLoading(false) } })
    return () => { active = false }
  }, [botId, hostScope])

  useEffect(() => {
    if (view !== 'settings') return
    let active = true
    setSaveError(null)
    void Promise.allSettled([
      invoke<{ providers?: unknown[]; models?: ModelOption[] }>('protocol_request', { method: 'provider.list', params: {} }),
      invoke<SkillsResponse>('protocol_request', { method: 'skill.list', params: {} }),
    ]).then(([providerResult, skillResult]) => {
      if (!active) return
      if (providerResult.status === 'fulfilled') setModels((providerResult.value.models ?? []).filter((model) => model.enabled !== false))
      else setSaveError(errorText(providerResult.reason))
      if (skillResult.status === 'fulfilled') setSkillCount((skillResult.value.skills ?? []).filter((skill) => skill.enabled && !skill.disabled_bot_ids?.includes(botId)).length)
      else if (skillResult.status === 'rejected') setSaveError((existing) => existing ?? errorText(skillResult.reason))
    })
    return () => { active = false }
  }, [view, botId])

  async function loadMoreHistory() {
    if (!cursor || historyLoading) return
    setHistoryLoading(true)
    setHistoryError(null)
    try {
      const result = await invoke<HistoryResponse>('protocol_request', { method: 'assignment.list', params: { bot_id: botId, status: TERMINAL_STATUSES, cursor, limit: 40 } })
      setHistory((current) => [...current, ...(result.items ?? [])])
      setCursor(result.next_cursor ?? null)
    } catch (error) { setHistoryError(errorText(error)) }
    finally { setHistoryLoading(false) }
  }

  async function updateBot(patch: Record<string, unknown>, manageBusy = true) {
    if (manageBusy) setBusy(true)
    setSaveError(null)
    try {
      const result = await invoke<{ bot: BotRecord }>('protocol_request', { method: 'bot.update', params: { bot_id: botId, patch } })
      setBot(result.bot)
      onBotUpdated(result.bot)
      return result.bot
    } catch (error) {
      setSaveError(errorText(error))
      throw error
    } finally { if (manageBusy) setBusy(false) }
  }

  async function saveSettings() {
    const avatar = draft.avatarKind === 'emoji'
      ? { kind: 'emoji', emoji: draft.avatarEmoji.trim() }
      : { kind: 'bean', color: draft.avatarColor }
    const patch: Record<string, unknown> = {
      name: draft.name.trim(), label: draft.label.trim(), description: draft.description.trim(), avatar,
      notifications: draft.notifications,
    }
    if (!bot.is_main) Object.assign(patch, { max_parallel: draft.maxParallel, tools: draft.tools, browser_mode: draft.browserMode })
    if (!bot.is_main && draft.model !== String(bot.model ?? settings?.models?.bot_default ?? '')) patch.model = draft.model || null
    setBusy(true)
    setSaveError(null)
    try {
      await updateBot(patch, false)
      if (bot.is_main) {
        const settingsPatch: Record<string, unknown> = { main_bot: { auto_create_project: draft.autoCreateProject } }
        if (draft.model !== String(bot.model ?? settings?.models?.main ?? '')) settingsPatch.models = { main: draft.model || null }
        const result = await invoke<{ settings: SettingsRecord }>('protocol_request', { method: 'settings.update', params: { patch: settingsPatch } })
        setSettings(result.settings)
      }
      setView('detail')
      setToast(t('bot.toast.saved'))
      window.setTimeout(() => setToast(''), 2200)
    } catch (error) { setSaveError(errorText(error)) }
    finally { setBusy(false) }
  }

  async function toggleFlag(flag: 'pinned' | 'hidden') {
    try {
      const updated = await updateBot({ [flag]: !bot[flag] })
      setDraft(makeDraft(updated, settings ?? undefined))
    } catch { /* error is shown in the panel */ }
  }

  async function duplicateBot() {
    const name = duplicateName.trim()
    if (!name) return
    setBusy(true)
    setSaveError(null)
    try {
      const result = await invoke<{ bot: BotRecord; dm_chat: { id: string } }>('protocol_request', { method: 'bot.duplicate', params: { bot_id: botId, name } })
      onBotDuplicated(result.bot, result.dm_chat.id)
      setDuplicateOpen(false)
      setDuplicateName('')
      setToast(t('bot.toast.duplicated'))
    } catch (error) { setSaveError(errorText(error)) }
    finally { setBusy(false) }
  }

  async function deleteBot() {
    setBusy(true)
    setSaveError(null)
    try {
      await invoke('protocol_request', { method: 'bot.delete', params: { bot_id: botId } })
      onBotDeleted(botId)
    } catch (error) { setSaveError(errorText(error)); setConfirmDelete(false) }
    finally { setBusy(false) }
  }

  function changeDraft<K extends keyof Draft>(key: K, value: Draft[K]) { setDraft((current) => ({ ...current, [key]: value })) }

  const avatarStyle = (color: number): CSSProperties => ({ background: `var(--avatar-${color})` })
  const historyProjectIds = [...new Set(history.map((assignment) => assignment.project_id).filter((id): id is string => Boolean(id)))]
  const title = view === 'settings' ? t('bot.page.settings') : bot.name

  return <aside className="bot-panel glass-panel" aria-label={title}>
    <header className="bot-panel-header drag-region" data-window-drag-region>
      {view === 'settings' ? <button className="bot-panel-icon" type="button" aria-label={t('common.back')} onClick={() => setView('detail')}><ChevronLeft size={15} /></button> : null}
      <strong>{title}</strong>
      <button className="bot-panel-icon" type="button" aria-label={t('common.close')} onClick={onClose}>×</button>
    </header>
    {view === 'detail' ? <div className="bot-panel-scroll">
      {loadError && <div className="bot-panel-error" role="alert"><AlertCircle size={14} />{loadError}</div>}
      <section className="bot-profile">
        <Avatar bot={bot} size={64} />
        <strong>{bot.name}</strong>
        <span>{[bot.label, modelLabel(bot.model, models, settings, bot.is_main)].filter(Boolean).join(' · ')}</span>
        <div className="bot-profile-actions">
          <button type="button" onClick={() => setView('settings')}><Settings2 size={13} />{t('bot.action.settings')}</button>
          <button type="button" onClick={() => onOpenComputer(botId)}><Monitor size={13} />{t('bot.action.screen')}</button>
          <button type="button" aria-label={t('bot.action.more')} onClick={() => setView('settings')}><Ellipsis size={15} /></button>
        </div>
      </section>

      <section className="bot-panel-section">
        <header><strong>{t('bot.groups')}</strong><span>{memberProjects.length}</span></header>
        {memberProjects.length ? memberProjects.map((project) => <button className="bot-group-row" key={project.id} type="button" onClick={() => { const chatId = chatForProject(project); if (chatId) onOpenChat(chatId) }}>
          <span className="bot-group-mark">{project.name.slice(0, 1)}</span><strong>{project.name}</strong><span className={`bot-project-status ${project.status ?? 'active'}`}>{t(`project.status.${project.status ?? 'active'}`)}</span><ChevronRight size={13} />
        </button>) : <p className="bot-panel-empty">{t('bot.groups.empty')}</p>}
      </section>

      {currentAssignments.length > 0 && <section className="bot-panel-section">
        <header><strong>{t('bot.active_tasks')}</strong><span>{currentAssignments.length}</span></header>
        {currentAssignments.slice(0, 4).map((assignment) => <button className="bot-task-row" type="button" key={assignment.id} onClick={() => onOpenTrace(assignment.id)}>
          <LoaderCircle size={13} className="bot-task-spinner" /><span><strong>{String(assignment.title ?? t('common.untitled'))}</strong><small>{t(`status.${String(assignment.status ?? 'working')}`)}</small></span><ArrowUpRight size={13} />
        </button>)}
      </section>}

      <section className="bot-panel-section">
        <header><strong>{t('bot.history')}</strong><span className="bot-history-filter"><select aria-label={t('bot.history.filter')} value={projectFilter} onChange={(event) => setProjectFilter(event.target.value)}><option value="">{t('common.all')}</option>{historyProjectIds.map((id) => <option value={id} key={id}>{projects.find((project) => project.id === id)?.name ?? id}</option>)}</select><ChevronDown size={12} /></span></header>
        {historyError && <div className="bot-panel-error" role="alert">{historyError}</div>}
        {visibleHistory.map((assignment) => <button className="bot-history-row" type="button" key={assignment.id} onClick={() => onOpenTrace(assignment.id)}>
          <span className={`bot-history-result ${assignment.status === 'done' ? 'done' : 'failed'}`}>{assignment.status === 'done' ? '✓' : '✗'}</span>
          <span className="bot-history-title"><strong>{projects.find((project) => project.id === assignment.project_id)?.name ?? t('bot.history.private')} · {assignment.title || assignment.instruction || t('common.untitled')}</strong><small>{shortDate(assignment.finished_at ?? assignment.created_at)} · {elapsed(assignment, t)}</small></span>
          <ChevronRight size={13} />
        </button>)}
        {historyLoading && <div className="bot-panel-loading"><LoaderCircle size={14} />{t('common.loading')}</div>}
        {!historyLoading && !visibleHistory.length && <p className="bot-panel-empty">{t('bot.history.empty')}</p>}
        {!historyLoading && cursor && <button type="button" className="bot-panel-more" onClick={() => void loadMoreHistory()}>{t('bot.history.more')}<ChevronRight size={13} /></button>}
      </section>

      <section className="bot-panel-section">
        <header><strong>{t('bot.week_usage')}</strong></header>
        {usageError ? <div className="bot-panel-error" role="alert">{usageError}</div> : <div className="bot-usage-cards">
          <div><small>{t('bot.usage.tokens')}</small><strong>{weekUsage ? formatNumber(Number(weekUsage.input_tokens ?? 0) + Number(weekUsage.output_tokens ?? 0)) : '—'}</strong></div>
          <div><small>{t('bot.usage.cost')}</small><strong>{formatCost(weekUsage?.cost)}</strong></div>
        </div>}
      </section>
    </div> : <div className="bot-panel-scroll bot-settings-scroll">
      {saveError && <div className="bot-panel-error" role="alert"><AlertCircle size={14} />{saveError}</div>}
      <section className="bot-avatar-editor">
        <Avatar bot={{ ...bot, avatar: draft.avatarKind === 'emoji' ? { kind: 'emoji', emoji: draft.avatarEmoji } : { kind: 'bean', color: draft.avatarColor } }} size={64} />
        <div className="bot-avatar-options" role="group" aria-label={t('bot.field.avatar')}>
          {AVATAR_COLORS.map((color) => <button type="button" key={color} className={`bot-avatar-color ${draft.avatarKind === 'bean' && draft.avatarColor === color ? 'selected' : ''}`} style={avatarStyle(color)} aria-label={`${t('bot.field.avatar')} ${color + 1}`} aria-pressed={draft.avatarKind === 'bean' && draft.avatarColor === color} onClick={() => { changeDraft('avatarKind', 'bean'); changeDraft('avatarColor', color) }} />)}
          <label className="bot-avatar-emoji"><span>{t('bot.avatar.other')}</span><input aria-label={t('bot.avatar.other')} maxLength={16} value={draft.avatarEmoji} onChange={(event) => { changeDraft('avatarKind', 'emoji'); changeDraft('avatarEmoji', event.target.value) }} /></label>
        </div>
      </section>

      <Field label={t('bot.field.name')}><input value={draft.name} maxLength={32} onChange={(event) => changeDraft('name', event.target.value)} /></Field>
      <Field label={t('bot.field.label')}><input value={draft.label} maxLength={64} onChange={(event) => changeDraft('label', event.target.value)} /></Field>
      <Field label={t('bot.field.rules')}><textarea value={draft.description} maxLength={4000} rows={4} onChange={(event) => changeDraft('description', event.target.value)} /></Field>
      <Field label={t('bot.field.model')}>
        <select value={draft.model} onChange={(event) => changeDraft('model', event.target.value)}><option value="">{t('bot.model.default')}</option>{models.map((model) => <option key={model.ref} value={model.ref}>{model.display_name ?? model.ref}</option>)}</select>
      </Field>
      {bot.is_main && <p className="bot-main-note">{t('bot.main.model_hint')}</p>}
      {bot.is_main ? <section className="bot-setting-row bot-main-rule"><span>{t('bot.main.rule')}</span><div className="bot-segmented"><button type="button" className={draft.autoCreateProject ? 'selected' : ''} onClick={() => changeDraft('autoCreateProject', true)}>{t('bot.main.rule.auto')}</button><button type="button" className={!draft.autoCreateProject ? 'selected' : ''} onClick={() => changeDraft('autoCreateProject', false)}>{t('bot.main.rule.ask')}</button></div></section> : <>
        <section className="bot-setting-row"><label htmlFor="bot-max-parallel">{t('bot.field.max_parallel')}</label><input id="bot-max-parallel" type="number" min={1} max={8} value={draft.maxParallel} onChange={(event) => changeDraft('maxParallel', Math.max(1, Math.min(8, Number(event.target.value) || 1)))} /></section>
        <section className="bot-setting-field"><strong>{t('bot.field.tools')}</strong><div className="bot-tools-card">{TOOL_KEYS.map((tool, index) => <label className="bot-tool-row" key={tool}><span>{t(`bot.tool.${tool === 'files' ? 'file' : tool}`)}</span><input type="checkbox" checked={draft.tools[tool] !== false} onChange={(event) => changeDraft('tools', { ...draft.tools, [tool]: event.target.checked })} aria-label={t(`bot.tool.${tool === 'files' ? 'file' : tool}`)} />{index < TOOL_KEYS.length - 1 && <i />}</label>)}</div></section>
        <button className="bot-setting-row bot-settings-link" type="button" onClick={() => onOpenSkills?.(botId)}><span>{t('bot.field.skills')}</span><small>{skillCount == null ? t('common.loading') : t('bot.field.skills_all').replace('{count}', String(skillCount))}</small><ChevronRight size={14} /></button>
        <section className="bot-setting-row"><label htmlFor="bot-browser-mode">{t('bot.field.browser_mode')}</label><select id="bot-browser-mode" value={draft.browserMode} onChange={(event) => changeDraft('browserMode', event.target.value as Draft['browserMode'])}><option value="headless">{t('bot.browser_mode.headless')}</option><option value="headless_profile">{t('bot.browser_mode.headless_profile')}</option><option value="attach">{t('bot.browser_mode.attach')}</option></select></section>
      </>}
      <label className="bot-setting-row bot-notifications"><span>{t('bot.field.notifications')}</span><input type="checkbox" checked={draft.notifications} onChange={(event) => changeDraft('notifications', event.target.checked)} /></label>
      {bot.is_main && <p className="bot-main-note">{t('bot.main.no_tools')}</p>}
      <button type="button" className="bot-save-button" disabled={busy || !draft.name.trim()} onClick={() => void saveSettings()}>{busy ? t('common.saving') : t('common.save')}</button>
      <div className="bot-admin-actions">
        {!bot.is_main && <>
          <button type="button" onClick={() => void toggleFlag('pinned')}>{t('bot.action.pin')}{bot.pinned ? ` · ${t('common.on')}` : ''}</button>
          <button type="button" onClick={() => void toggleFlag('hidden')}>{t('bot.action.hide')}{bot.hidden ? ` · ${t('common.on')}` : ''}</button>
          <button type="button" onClick={() => { setDuplicateName(`${bot.name} 2`); setDuplicateOpen(true) }}>{t('bot.action.duplicate')}</button>
          <button type="button" className="danger" onClick={() => setConfirmDelete(true)}><Trash2 size={13} />{t('bot.action.delete')}</button>
        </>}
      </div>
    </div>}
    {duplicateOpen && <div className="bot-inline-dialog" role="dialog" aria-label={t('bot.action.duplicate')}><label>{t('bot.duplicate.name')}<input autoFocus maxLength={32} value={duplicateName} onChange={(event) => setDuplicateName(event.target.value)} /></label><div><button type="button" onClick={() => setDuplicateOpen(false)}>{t('common.cancel')}</button><button type="button" disabled={busy || !duplicateName.trim()} onClick={() => void duplicateBot()}>{t('bot.action.duplicate')}</button></div></div>}
    {confirmDelete && <div className="bot-inline-dialog" role="alertdialog" aria-label={t('bot.action.delete')}><p>{t('bot.delete.confirm').replace('{bot}', bot.name)}</p>{saveError && <div className="bot-panel-error" role="alert">{saveError}</div>}<div><button type="button" onClick={() => setConfirmDelete(false)}>{t('common.cancel')}</button><button type="button" className="danger" disabled={busy} onClick={() => void deleteBot()}>{t('bot.action.delete')}</button></div></div>}
    {toast && <div className="bot-panel-toast" role="status"><Sparkles size={13} />{toast}</div>}
  </aside>
}

function Avatar({ bot, size }: { bot: BotRecord; size: number }) {
  const avatar = bot.avatar
  const style: CSSProperties = { width: size, height: size, background: `var(--avatar-${Number.isInteger(avatar?.color) ? avatar?.color : 0})` }
  return <span className={`bot-panel-avatar ${bot.is_main ? 'main' : ''}`} style={style} aria-hidden="true">{avatar?.kind === 'emoji' && avatar.emoji ? avatar.emoji : Array.from(bot.name || 'B')[0]}{bot.is_main && <svg className="bot-panel-glasses" viewBox="0 0 100 100"><rect x="24" y="37" width="25" height="22" rx="7"/><rect x="51" y="37" width="25" height="22" rx="7"/><path d="M49 45q1-2 2 0M24 44 12 41M76 44l12-3"/></svg>}</span>
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return <label className="bot-setting-field"><strong>{label}</strong>{children}</label>
}

function modelLabel(ref: string | null | undefined, models: ModelOption[], settings: SettingsRecord | null, isMain?: boolean) {
  const effective = ref ?? (isMain ? settings?.models?.main : settings?.models?.bot_default)
  return models.find((model) => model.ref === effective)?.display_name ?? effective ?? ''
}
