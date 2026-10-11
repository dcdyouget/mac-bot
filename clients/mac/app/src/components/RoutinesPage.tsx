import { useEffect, useMemo, useRef, useState, type CSSProperties, type FormEvent } from 'react'
import { AlertCircle, Check, Clock3, LoaderCircle, Play, Plus, Trash2, X } from 'lucide-react'
import './RoutinesPage.css'

export type RoutineSchedule = { cron: string; label: string }

export type RoutineRun = {
  id: string
  routine_id: string
  assignment_id: string | null
  trigger: 'schedule' | 'test'
  status: 'running' | 'done' | 'failed' | 'skipped'
  started_at: string
  finished_at: string | null
  error: string | null
}

export type RoutineRecord = {
  id: string
  bot_id: string
  project_id: string | null
  name: string
  instructions: string
  schedules: RoutineSchedule[]
  timezone: string
  enabled: boolean
  next_run_at: string | null
  last_run: RoutineRun | null
  created_at: string
  updated_at: string
}

export type RoutineBot = { id: string; name: string; avatar_color?: string; is_main?: boolean }
export type RoutineProject = { id: string; name: string; chat_id?: string }

export type RoutineInput = {
  bot_id: string
  project_id?: string
  name: string
  instructions: string
  schedules: RoutineSchedule[]
  timezone: string
}

export type RoutinePatch = Partial<Omit<RoutineInput, 'project_id'>> & { project_id?: string | null }

export type RoutineCopyKey =
  | 'common.all' | 'common.new' | 'common.add' | 'common.save' | 'common.cancel' | 'common.delete' | 'common.retry' | 'common.enable'
  | 'routines.title' | 'routines.filter.running' | 'routines.filter.paused' | 'routines.paused'
  | 'routines.next_run' | 'routines.test_run' | 'routines.field.name' | 'routines.field.bot'
  | 'routines.field.instruction' | 'routines.field.when' | 'routines.field.add_time' | 'routines.field.timezone'
  | 'routines.field.deliver_to' | 'routines.deliver.dm' | 'routines.deliver.group' | 'routines.history'
  | 'routines.run.ok' | 'routines.run.trace' | 'routines.empty' | 'routines.loading' | 'routines.error'
  | 'routines.field.cron' | 'routines.group.choose' | 'routines.delete.confirm'
  | 'routines.run.running' | 'routines.run.failed' | 'routines.run.skipped' | 'routines.run.duration'
  | 'routines.schedule.default_weekdays' | 'routines.schedule.default_daily'

type RoutineForm = Omit<RoutineInput, 'project_id'> & { project_id: string | null }
type RoutineFilter = 'all' | 'running' | 'paused'

type Props = {
  routines: RoutineRecord[]
  selectedRoutineId?: string | null
  selectionRequest?: number
  bots: RoutineBot[]
  projects: RoutineProject[]
  runs: Record<string, RoutineRun[]>
  loading?: boolean
  error?: string | null
  onRetry?: () => void | Promise<void>
  onCreate: (routine: RoutineInput) => Promise<void>
  onUpdate: (routineId: string, patch: RoutinePatch) => Promise<void>
  onSetEnabled: (routineId: string, enabled: boolean) => Promise<void>
  onTestRun: (routineId: string) => Promise<RoutineRun>
  onLoadRuns?: (routineId: string) => Promise<RoutineRun[]>
  onDelete: (routineId: string) => Promise<void>
  onOpenTrace: (assignmentId: string, run: RoutineRun) => void
  t: (key: RoutineCopyKey) => string
}

const blankForm = (botId: string, t: Props['t']): RoutineForm => ({
  bot_id: botId,
  project_id: null,
  name: '',
  instructions: '',
  schedules: [{ cron: '0 9 * * 1-5', label: t('routines.schedule.default_weekdays') }],
  timezone: Intl.DateTimeFormat().resolvedOptions().timeZone || 'Asia/Shanghai',
})

export function RoutinesPage(props: Props) {
  const [filter, setFilter] = useState<RoutineFilter>('all')
  const [selectedId, setSelectedId] = useState<string | null>(props.selectedRoutineId ?? props.routines[0]?.id ?? null)
  const [creating, setCreating] = useState(false)
  const [form, setForm] = useState<RoutineForm>(() => blankForm(props.bots[0]?.id ?? '', props.t))
  const [busy, setBusy] = useState(false)
  const [actionError, setActionError] = useState('')
  const [confirmDelete, setConfirmDelete] = useState(false)
  const [testRuns, setTestRuns] = useState<RoutineRun[]>([])
  const [loadedRuns, setLoadedRuns] = useState<Record<string, RoutineRun[]>>({})
  const appliedSelectionRequest = useRef<number | undefined>(undefined)

  const active = props.routines.find((routine) => routine.id === selectedId) ?? (props.selectedRoutineId ? null : props.routines[0] ?? null)
  const activeId = active?.id
  const onLoadRuns = props.onLoadRuns
  const activeBot = props.bots.find((bot) => bot.id === active?.bot_id)
  const visibleRoutines = useMemo(() => props.routines.filter((routine) => (
    filter === 'all' || (filter === 'running' ? routine.enabled : !routine.enabled)
  )), [filter, props.routines])
  const runHistory = useMemo(() => {
    if (!active) return []
    const byId = new Map<string, RoutineRun>()
    for (const run of [...(props.runs[active.id] ?? []), ...(loadedRuns[active.id] ?? []), ...testRuns.filter((run) => run.routine_id === active.id)]) byId.set(run.id, run)
    return [...byId.values()].sort((left, right) => Date.parse(right.started_at) - Date.parse(left.started_at)).slice(0, 20)
  }, [active, props.runs, loadedRuns, testRuns])

  useEffect(() => {
    if (!props.selectedRoutineId || appliedSelectionRequest.current === props.selectionRequest) return
    const routine = props.routines.find((item) => item.id === props.selectedRoutineId)
    if (!routine) return
    appliedSelectionRequest.current = props.selectionRequest
    setCreating(false)
    setSelectedId(routine.id)
    setForm(formFrom(routine))
    setActionError('')
    setConfirmDelete(false)
  }, [props.selectedRoutineId, props.selectionRequest, props.routines])

  useEffect(() => {
    if (creating) return
    if (active) setForm(formFrom(active))
  }, [active, creating])

  useEffect(() => {
    if (!activeId || !onLoadRuns) return
    let current = true
    void onLoadRuns(activeId).then((runs) => {
      if (current) setLoadedRuns((items) => ({ ...items, [activeId]: runs }))
    }).catch((reason: unknown) => { if (current) setActionError(String(reason)) })
    return () => { current = false }
  }, [activeId, onLoadRuns])

  if (props.loading) return <main className="routines-page"><RoutineLoading t={props.t} /></main>

  const updateForm = (patch: Partial<RoutineForm>) => setForm((current) => ({ ...current, ...patch }))
  const updateSchedule = (index: number, patch: Partial<RoutineSchedule>) => setForm((current) => ({
    ...current,
    schedules: current.schedules.map((schedule, scheduleIndex) => scheduleIndex === index ? { ...schedule, ...patch } : schedule),
  }))

  async function runAction(action: () => Promise<void>) {
    setBusy(true)
    setActionError('')
    try { await action() } catch (reason) { setActionError(String(reason)) } finally { setBusy(false) }
  }

  async function save(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (busy) return
    const values = {
      ...form,
      name: form.name.trim(),
      instructions: form.instructions.trim(),
      schedules: form.schedules.map((schedule) => ({ cron: schedule.cron.trim(), label: schedule.label.trim() })),
    }
    await runAction(async () => {
      if (creating) {
        const { project_id, ...createFields } = values
        await props.onCreate({ ...createFields, ...(project_id ? { project_id } : {}) })
        setCreating(false)
      } else if (active) {
        await props.onUpdate(active.id, values)
      }
    })
  }

  async function testRun() {
    if (!active) return
    setBusy(true)
    setActionError('')
    try {
      const run = await props.onTestRun(active.id)
      setTestRuns((current) => [...current, run])
    }
    catch (reason) { setActionError(String(reason)) }
    finally { setBusy(false) }
  }

  async function deleteRoutine() {
    if (!active) return
    await runAction(async () => {
      await props.onDelete(active.id)
      setConfirmDelete(false)
      setSelectedId(props.routines.find((routine) => routine.id !== active.id)?.id ?? null)
    })
  }

  function startCreating() {
    setCreating(true)
    setSelectedId(null)
    setActionError('')
    setConfirmDelete(false)
    setForm(blankForm(props.bots[0]?.id ?? '', props.t))
  }

  function selectRoutine(routine: RoutineRecord) {
    setCreating(false)
    setSelectedId(routine.id)
    setForm(formFrom(routine))
    setActionError('')
    setConfirmDelete(false)
  }

  const selectedBot = props.bots.find((bot) => bot.id === form.bot_id)
  const canSave = Boolean(form.name.trim() && form.instructions.trim() && form.bot_id && form.schedules.length && form.schedules.every((schedule) => schedule.cron.trim() && schedule.label.trim()) && (form.project_id === null || props.projects.some((project) => project.id === form.project_id)))

  return <main className="routines-page">
    <aside className="routines-list glass-panel">
      <header className="routines-list-toolbar">
        <h1>{props.t('routines.title')}</h1>
        <button type="button" className="routines-add-button" onClick={startCreating}><Plus size={14} />{props.t('common.new')}</button>
      </header>
      <div className="routines-filter" role="tablist">
        <button type="button" role="tab" aria-selected={filter === 'all'} className={filter === 'all' ? 'selected' : ''} onClick={() => setFilter('all')}>{props.t('common.all')}</button>
        <button type="button" role="tab" aria-selected={filter === 'running'} className={filter === 'running' ? 'selected' : ''} onClick={() => setFilter('running')}>{props.t('routines.filter.running')}</button>
        <button type="button" role="tab" aria-selected={filter === 'paused'} className={filter === 'paused' ? 'selected' : ''} onClick={() => setFilter('paused')}>{props.t('routines.filter.paused')}</button>
      </div>
      <div className="routines-list-items">
        {visibleRoutines.map((routine, index) => <RoutineListRow
          key={routine.id}
          routine={routine}
          bot={props.bots.find((bot) => bot.id === routine.bot_id)}
          selected={routine.id === active?.id && !creating}
          avatarColor={avatarColor(props.bots.findIndex((bot) => bot.id === routine.bot_id), index)}
          t={props.t}
          onClick={() => selectRoutine(routine)}
        />)}
        {!visibleRoutines.length && <div className="routines-empty-list">{props.t('routines.empty')}</div>}
      </div>
    </aside>

    <section className="routines-detail">
      {props.error && <div className="routines-error" role="alert"><AlertCircle size={15} /><span>{props.error}</span>{props.onRetry && <button type="button" onClick={() => void props.onRetry?.()}>{props.t('common.retry')}</button>}</div>}
      {active || creating ? <>
        <header className="routines-detail-toolbar glass-toolbar">
          <div className="routines-detail-heading">
            <strong>{creating ? props.t('routines.title') : active?.name}</strong>
            {!creating && active && <small>{nextRunText(active, activeBot?.name ?? '', props.t)}</small>}
          </div>
          {!creating && active && <>
            <button type="button" className="routines-action-button" disabled={busy} onClick={() => void testRun()}><Play size={14} />{props.t('routines.test_run')}</button>
            <button type="button" className="routines-action-button danger" disabled={busy} onClick={() => setConfirmDelete(true)}><Trash2 size={14} />{props.t('common.delete')}</button>
            <span className="routines-enable-label">{props.t('common.enable')}</span>
            <button type="button" role="switch" aria-checked={active.enabled} aria-label={props.t('common.enable')} className={`routines-switch ${active.enabled ? 'on' : ''}`} disabled={busy} onClick={() => void runAction(() => props.onSetEnabled(active.id, !active.enabled))}><span /></button>
          </>}
        </header>
        <div className="routines-detail-scroll">
          <form className="routines-form" onSubmit={(event) => void save(event)}>
            <label className="routines-field"><span>{props.t('routines.field.name')}</span><input className="routines-input large" required value={form.name} onChange={(event) => updateForm({ name: event.target.value })} /></label>
            <label className="routines-field"><span>{props.t('routines.field.bot')}</span><select className="routines-input large" required value={form.bot_id} onChange={(event) => updateForm({ bot_id: event.target.value })}>
              {props.bots.map((bot) => <option key={bot.id} value={bot.id}>{bot.name}</option>)}
            </select></label>
            <label className="routines-field"><span>{props.t('routines.field.instruction')}</span><textarea className="routines-input routines-instructions" required value={form.instructions} onChange={(event) => updateForm({ instructions: event.target.value })} /></label>
            <section className="routines-field"><span>{props.t('routines.field.when')}</span><div className="routines-schedules">
              {form.schedules.map((schedule, index) => <div className="routines-schedule-row" key={`${index}-${schedule.cron}`}>
                <Clock3 size={14} />
                <input aria-label={props.t('routines.field.when')} className="routines-input schedule-label" value={schedule.label} onChange={(event) => updateSchedule(index, { label: event.target.value })} />
                <input aria-label={props.t('routines.field.cron')} className="routines-input schedule-cron" value={schedule.cron} onChange={(event) => updateSchedule(index, { cron: event.target.value })} />
                <button type="button" className="routines-icon-button" aria-label={props.t('common.delete')} disabled={form.schedules.length === 1} onClick={() => updateForm({ schedules: form.schedules.filter((_, scheduleIndex) => scheduleIndex !== index) })}><X size={14} /></button>
              </div>)}
                <button type="button" className="routines-add-time" onClick={() => updateForm({ schedules: [...form.schedules, { cron: '0 9 * * *', label: props.t('routines.schedule.default_daily') }] })}><Plus size={14} />{props.t('routines.field.add_time')}</button>
            </div>
            <label className="routines-timezone">{props.t('routines.field.timezone').replace('{tz}', '')}<input className="routines-input" value={form.timezone} onChange={(event) => updateForm({ timezone: event.target.value })} /></label></section>
            <section className="routines-field"><span>{props.t('routines.field.deliver_to')}</span>
              <div className="routines-delivery-segment">
                <button type="button" className={form.project_id === null ? 'selected' : ''} onClick={() => updateForm({ project_id: null })}>{props.t('routines.deliver.dm').replace('{bot}', selectedBot?.name ?? '')}</button>
                <button type="button" className={form.project_id !== null ? 'selected' : ''} disabled={!props.projects.length} onClick={() => updateForm({ project_id: form.project_id ?? props.projects[0]?.id ?? null })}>{props.t('routines.deliver.group')}</button>
              </div>
              {form.project_id !== null && <select className="routines-input routines-project-select" required value={form.project_id} onChange={(event) => updateForm({ project_id: event.target.value })}>
                <option value="">{props.t('routines.group.choose')}</option>{props.projects.map((project) => <option key={project.id} value={project.id}>{project.name}</option>)}
              </select>}
            </section>
            {actionError && <div className="routines-error" role="alert"><AlertCircle size={15} />{actionError}</div>}
            <div className="routines-form-actions"><button type="button" className="routines-cancel-button" disabled={busy} onClick={() => { if (creating) { setCreating(false); setSelectedId(props.routines[0]?.id ?? null) } else if (active) setForm(formFrom(active)); setActionError('') }}>{props.t('common.cancel')}</button><button type="submit" className="routines-save-button" disabled={busy || !canSave}>{busy ? <LoaderCircle className="routines-spin" size={14} /> : <Check size={14} />}{props.t('common.save')}</button></div>
          </form>
          {!creating && active && <section className="routines-history-section"><h2>{props.t('routines.history')}</h2>
            {runHistory.length ? <div className="routines-history-card"><table className="routines-history-table"><tbody>{runHistory.map((run) => <tr key={run.id}>
              <td>{formatDate(run.started_at)}</td>
              <td className={`routine-run-status status-${run.status}`}>{run.status === 'done' ? props.t('routines.run.ok') : props.t(runStatusKey(run.status))}{run.error && <small>{run.error}</small>}</td>
              <td className="routine-duration">{props.t('routines.run.duration').replace('{duration}', duration(run.started_at, run.finished_at))}</td>
              <td>{run.assignment_id && <button type="button" className="routines-trace-button" onClick={() => props.onOpenTrace(run.assignment_id!, run)}>{props.t('routines.run.trace')}</button>}</td>
            </tr>)}</tbody></table></div> : <div className="routines-empty-history">{props.t('routines.empty')}</div>}
          </section>}
        </div>
      </> : <div className="routines-page-empty"><Clock3 size={24} /><span>{props.t('routines.empty')}</span></div>}
      {confirmDelete && active && <div className="routines-dialog-backdrop" role="presentation"><section className="routines-confirm-dialog" role="alertdialog" aria-modal="true" aria-label={props.t('common.delete')}>
        <h2>{props.t('common.delete')}</h2><p>{props.t('routines.delete.confirm').replace('{name}', active.name)}</p>
        <div><button type="button" disabled={busy} onClick={() => setConfirmDelete(false)}>{props.t('common.cancel')}</button><button type="button" className="danger" disabled={busy} onClick={() => void deleteRoutine()}>{props.t('common.delete')}</button></div>
      </section></div>}
    </section>
  </main>
}

function RoutineListRow({ routine, bot, selected, avatarColor: color, t, onClick }: { routine: RoutineRecord; bot?: RoutineBot; selected: boolean; avatarColor: string; t: Props['t']; onClick: () => void }) {
  const style = { '--routine-avatar': color } as CSSProperties
  return <button type="button" className={`routines-list-row ${selected ? 'selected' : ''}`} aria-current={selected ? 'page' : undefined} onClick={onClick}>
    <span className="routines-avatar" style={style}>{initials(bot?.name ?? '?')}</span>
    <span className="routines-row-copy"><span className="routines-row-heading"><strong>{routine.name}</strong>{!routine.enabled && <small>{t('routines.paused')}</small>}</span><span className="routines-row-subtitle"><Clock3 size={13} />{routine.schedules[0]?.label ?? ''} · {bot?.name ?? '—'}</span></span>
  </button>
}

function RoutineLoading({ t }: { t: Props['t'] }) {
  return <div className="routines-loading"><LoaderCircle className="routines-spin" size={20} /><span>{t('routines.loading')}</span></div>
}

function formFrom(routine: RoutineRecord): RoutineForm {
  return { bot_id: routine.bot_id, project_id: routine.project_id, name: routine.name, instructions: routine.instructions, schedules: routine.schedules.map((schedule) => ({ ...schedule })), timezone: routine.timezone }
}

function avatarColor(botIndex: number, rowIndex: number) {
  const index = botIndex < 0 ? rowIndex : botIndex
  return `var(--avatar-${index % 10})`
}

function initials(name: string) {
  return [...name.trim()].slice(0, 2).join('') || '?'
}

function nextRunText(routine: RoutineRecord, botName: string, t: Props['t']) {
  return t('routines.next_run').replace('{time}', routine.next_run_at ? formatDate(routine.next_run_at, routine.timezone) : '—').replace('{bot}', botName)
}

function formatDate(value: string, timeZone?: string) {
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? value : new Intl.DateTimeFormat('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', ...(timeZone ? { timeZone } : {}) }).format(date)
}

function duration(start: string, finish: string | null) {
  const startMs = Date.parse(start)
  const endMs = finish ? Date.parse(finish) : Date.now()
  if (!Number.isFinite(startMs) || !Number.isFinite(endMs) || endMs < startMs) return '—'
  const seconds = Math.floor((endMs - startMs) / 1000)
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`
}

function runStatusKey(status: RoutineRun['status']): RoutineCopyKey {
  return status === 'running' ? 'routines.run.running' : status === 'failed' ? 'routines.run.failed' : 'routines.run.skipped'
}
