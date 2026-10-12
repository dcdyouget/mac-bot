import { useState } from 'react'
import { AlertTriangle, Check, ChevronRight, Circle, Clock3, File, Folder, Globe, MessageCircle, Reply, Target, X, Zap } from 'lucide-react'
import { t } from '../i18n'
import type { Json } from '../types'
import { getTaskCardStatus } from './messageBlocksTaskCard'
import { getSkillMessageDelivery, parseForcedSkillText, type LoadedSkillRecord, type SkillMessageDelivery } from './skillMessage.model'

export type ArtifactRef = { artifact_id: string; title: string; path_or_url: string; bot_id?: string; kind?: string }

type ProjectData = { id?: string; chat_id?: string; name?: string; status?: string; done_at?: string | null; members?: { bot_id?: string }[] }
type BotMap = Record<string, { id: string; name?: string; is_main?: boolean }>

export function SkillMessageText({ text, knownSkillNames }: { text: string; knownSkillNames: string[] }) {
  const parsed = parseForcedSkillText(text, knownSkillNames)
  if (!parsed) return <>{text}</>
  return <span className="skill-message-line"><span className="skill-message-chip"><Zap size={13} />/{parsed.name}</span>{parsed.text && <span className="skill-message-body">{parsed.text}</span>}</span>
}

export function SkillMessageDeliveryStatus({ deliveries, bots }: { deliveries: SkillMessageDelivery[]; bots: BotMap }) {
  const values = getSkillMessageDelivery(deliveries, Object.fromEntries(Object.values(bots).map((bot) => [bot.id, bot.name ?? t('common.bot')])))
  if (!values.length) return null
  return <div className="message-send-status skill-message-status">{values.map(({ state, botName }, index) => <span className={`skill-message-delivery skill-delivery-${state}`} key={`${state}-${botName}-${index}`}>
    {state === 'queued' ? <Clock3 size={11} /> : <Check size={11} />}
    {state === 'queued' ? t('chat.delivery.queued') : state === 'read' ? t('chat.delivery.read').replace('{bot}', botName) : t('chat.status.sent')}
  </span>)}</div>
}

export function SkillMessageLoadedStatus({ loadedSkills, bots }: { loadedSkills: LoadedSkillRecord[]; bots: BotMap }) {
  if (!loadedSkills.length) return null
  return <div className="message-send-status skill-message-status">{loadedSkills.map((loadedSkill, index) => <span className="skill-message-delivery skill-delivery-read" key={`${loadedSkill.name}-${loadedSkill.bot_id}-${loadedSkill.assignment_id ?? ''}-${index}`}>
    <Check size={11} />{t('chat.slash.loaded').replace('{name}', loadedSkill.name)}<small>{bots[loadedSkill.bot_id]?.name ?? loadedSkill.bot_id}</small>
  </span>)}</div>
}

export function ProjectCard({ projectId, context, note, project, bots, onOpen }: { projectId: string; context?: string; note?: string | null; project?: ProjectData; bots: BotMap; onOpen: (projectId: string, chatId?: string) => Promise<void> }) {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  if (!projectId) return <FallbackCard text={t('common.unsupported_block')} />
  const members = (project?.members ?? []).map((member) => bots[member.bot_id ?? '']?.name).filter((name): name is string => Boolean(name))
  const status = context === 'created' ? 'new' : project?.status ?? 'active'
  const statusText = t(status === 'new' ? 'project.status.new' : status === 'review' ? 'project.status.review' : status === 'done' ? 'project.status.done' : status === 'archived' ? 'project.status.archived' : 'project.status.active')
  const chatId = project?.chat_id ?? ''
  return <div className="project-card-wrap"><button type="button" className="project-card" disabled={busy} onClick={() => void open()}>
    {context === 'created' && <div className="project-card-created">{t('block.project_card.created')}</div>}
    <div className="project-card-heading"><span className="project-card-avatar"><MessageCircle size={19} /></span><span className="project-card-main"><strong>{project?.name ?? projectId}</strong><small>{t('block.project_card.subtitle').replace('{members}', members.join('、'))}</small></span><span className={`project-status status-${status}`}>{statusText}</span><ChevronRight size={15} /></div>
    {note && <p className="project-card-note">{note}</p>}
    <div className="project-card-foot"><MessageCircle size={13} />{t('block.project_card.enter')}</div>
  </button>{error && <p className="project-card-error" role="alert">{error}</p>}</div>

  async function open() {
    setBusy(true)
    setError('')
    try { await onOpen(projectId, chatId) } catch (reason) { setError(String(reason)) } finally { setBusy(false) }
  }
}

export function FallbackCard({ text }: { text: string }) {
  return <div className="block-fallback"><AlertTriangle size={14} /><span>{text}<small>{t('block.fallback.hint')}</small></span></div>
}

export function TaskCard({ assignment, groupName, botName, onDetail }: { assignment?: Record<string, Json>; groupName?: string; botName?: string; onDetail: (id: string) => void }) {
  if (!assignment) return null
  const id = String(assignment.id ?? '')
  const title = String(assignment.title ?? assignment.bot_id ?? '')
  const status = String(assignment.status ?? 'working')
  const { statusKey, recovered } = getTaskCardStatus(status, assignment.recovered_at)
  const elapsed = formatDuration(assignment.started_at ?? assignment.created_at, assignment.finished_at)
  const elapsedValue = durationValue(assignment.started_at ?? assignment.created_at, assignment.finished_at)
  const reason = status === 'queued' && assignment.queue_reason ? t('block.task_card.queued_reason').replace('{task}', String(assignment.queue_reason)) : typeof assignment.failure_reason === 'string' ? assignment.failure_reason : ''
  if (status === 'done') return <div className="task-card-done-line"><Check size={14} /><span>{t('block.task_card.done_line').replace('{bot}', botName ?? t('common.bot')).replace('{title}', title).replace('{duration}', elapsedValue || '—')}{recovered && ` · ${t('status.recovered')}`}</span><button type="button" className="task-detail-button" onClick={() => id && onDetail(id)}>{t('common.details')}</button></div>
  return <div className={`task-card task-${status}`}>
    <span className={`task-card-icon task-icon-${status}`}>{status === 'done' ? <Check size={15} /> : status === 'failed' || status === 'blocked' || status === 'cancelled' ? <X size={15} /> : status === 'queued' ? <Clock3 size={15} /> : <Circle size={13} />}</span>
    <span className="task-card-copy"><strong>{t('block.task_card.title').replace('{title}', title)}</strong>{groupName && <small>{groupName}</small>}<small>{statusText(statusKey)}{elapsed && ` · ${elapsed}`}{reason && ` · ${reason}`}{recovered && ` · ${t('status.recovered')}`}</small></span>
    <button type="button" className="task-detail-button" onClick={() => id && onDetail(id)}>{t('trace.show')}</button>
    {status === 'working' && <i className="task-progress-bar" />}
  </div>
}

export function ProgressRow({ text, botName }: { text: string; botName: string }) {
  return <div className="progress-row"><span className="progress-dot" />{botName}：{text}</div>
}

export function DelegationLine({ botName, title, instruction }: { botName: string; title?: string; instruction?: string }) {
  return <div className="delegation-line"><Reply size={14} /><span>{t('block.delegation.text').replace('{bot}', botName)}{instruction ? `：${instruction}` : title ? ` · ${title}` : ''}</span></div>
}

export function SystemNotice({ code, text, botName }: { code: string; text: string; botName: string }) {
  const value = text || (code === 'member_added' ? t('block.system.member_added').replace('{bot}', botName) : code === 'announcement_updated' ? t('block.system.announcement_updated').replace('{who}', botName) : code === 'created' ? t('block.system.created').replace('{members}', '') : '')
  return <div className="system-notice">{value}</div>
}

export function CompletionCard({ block, bots, onPreview }: { block: Record<string, Json>; bots: BotMap; onPreview: (artifact: ArtifactRef) => void }) {
  const summary = String(block.summary ?? '')
  const artifacts = Array.isArray(block.artifacts) ? block.artifacts.filter(isArtifact) : []
  const next = Array.isArray(block.next) ? block.next.filter(isRecord) : []
  return <section className="completion-card">
    <div className="completion-heading"><Check size={14} /><strong>{t('block.completion.done')}</strong><span>{summary}</span></div>
    {artifacts.length > 0 && <ArtifactRows artifacts={artifacts} bots={bots} onPreview={onPreview} />}
    {next.length > 0 && <div className="completion-next">{next.map((item, index) => <div key={index}><ChevronRight size={13} /><b>@{bots[String(item.bot_id ?? '')]?.name ?? t('common.bot')}</b> {String(item.instruction ?? '')}</div>)}</div>}
    {block.notify_main === true && <div className="completion-next"><div><ChevronRight size={13} /><b>@{Object.values(bots).find((bot) => bot.is_main)?.name ?? t('common.bot')}</b> {t('block.completion.notify_main')}</div></div>}
  </section>
}

export function ReviewCard({ projectName, state, confirmedAt, artifacts, bots, onPreview, onOpenProject, onConfirm, onRequestChanges }: { projectName: string; state: string; confirmedAt?: string | null; artifacts: ArtifactRef[]; bots: BotMap; onPreview: (artifact: ArtifactRef) => void; onOpenProject: () => Promise<void>; onConfirm: () => Promise<void>; onRequestChanges: (text: string) => Promise<void> }) {
  const [dialogOpen, setDialogOpen] = useState(false)
  const [text, setText] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  async function run(action: () => Promise<void>) {
    setBusy(true)
    setError('')
    try { await action() } catch (reason) { setError(String(reason)) } finally { setBusy(false) }
  }
  return <section className="review-card">
    <div className="review-heading"><Target size={15} /><strong>{t('block.review_card.title').replace('{project}', projectName)}</strong></div>
    {artifacts.length > 0 && <ArtifactRows artifacts={artifacts} bots={bots} onPreview={onPreview} compact />}
    {state === 'confirmed' ? <div className="review-result confirmed"><Check size={14} />{confirmedAt ? t('block.review_card.confirmed').replace('{time}', formatTime(confirmedAt)) : t('block.review_card.confirmed').replace(' · {time}', '')}</div>
      : state === 'changes_requested' ? <div className="review-result">{t('block.review_card.changes_requested')}</div>
      : <div className="review-actions"><button type="button" className="review-confirm" disabled={busy} onClick={() => void run(onConfirm)}>{t('block.review_card.confirm')}</button><button type="button" disabled={busy} onClick={() => { setText(''); setDialogOpen(true) }}>{t('block.review_card.request_changes')}</button><button type="button" className="review-open" disabled={busy} onClick={() => void run(onOpenProject)}>{t('block.project_card.enter')}</button></div>}
    {dialogOpen && <div className="review-dialog-backdrop"><form className="review-dialog" onSubmit={(event) => { event.preventDefault(); void run(async () => { await onRequestChanges(text.trim()); setDialogOpen(false) }) }}>
      <h3>{t('block.review_card.changes_dialog_title').replace('{project}', projectName)}</h3><p>{t('block.review_card.changes_dialog_hint')}</p><textarea autoFocus value={text} onChange={(event) => setText(event.target.value)} required /><div><button type="button" disabled={busy} onClick={() => setDialogOpen(false)}>{t('common.close')}</button><button type="submit" disabled={busy || !text.trim()}>{t('block.review_card.request_changes')}</button></div>{error && <small role="alert">{error}</small>}
    </form></div>}
    {error && !dialogOpen && <p className="review-error" role="alert">{error}</p>}
  </section>
}

export function ArtifactRows({ artifacts, bots, onPreview, compact = false }: { artifacts: ArtifactRef[]; bots: BotMap; onPreview: (artifact: ArtifactRef) => void; compact?: boolean }) {
  return <div className="artifact-rows">{artifacts.map((artifact) => {
    const Icon = artifact.kind === 'dir' ? Folder : artifact.kind === 'url' ? Globe : File
    return <div className="artifact-row" key={artifact.artifact_id}>
      <span className="artifact-icon"><Icon size={16} /></span><span className="artifact-meta"><span className="artifact-title">{artifact.title}<small>{bots[artifact.bot_id ?? '']?.name ?? ''}</small></span><small className="artifact-path">{artifact.path_or_url}</small></span>
      {!compact && <button type="button" className="artifact-preview" onClick={() => onPreview(artifact)}>{t('artifact.preview.tabs.preview')}</button>}
    </div>
  })}</div>
}

function isRecord(value: unknown): value is Record<string, Json> { return Boolean(value) && typeof value === 'object' && !Array.isArray(value) }
function isArtifact(value: unknown): value is ArtifactRef { return isRecord(value) && typeof value.artifact_id === 'string' && typeof value.title === 'string' && typeof value.path_or_url === 'string' }
function formatDuration(startValue: Json | undefined, endValue: Json | undefined) {
  const duration = durationValue(startValue, endValue)
  if (!duration) return ''
  return t(endValue ? 'block.task_card.took' : 'block.task_card.elapsed').replace('{duration}', duration)
}
function durationValue(startValue: Json | undefined, endValue: Json | undefined) {
  if (typeof startValue !== 'string') return ''
  const start = Date.parse(startValue)
  if (!Number.isFinite(start)) return ''
  const seconds = Math.max(0, Math.floor(((typeof endValue === 'string' ? Date.parse(endValue) : Date.now()) - start) / 1000))
  return seconds < 60 ? `${seconds} 秒` : seconds < 3600 ? `${Math.floor(seconds / 60)} 分` : `${Math.floor(seconds / 3600)} 小时 ${Math.floor(seconds % 3600 / 60)} 分`
}
function formatTime(value?: string | null) {
  if (!value) return ''
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? '' : date.toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' })
}
function statusText(key: string) { return t(key as Parameters<typeof t>[0]) }
