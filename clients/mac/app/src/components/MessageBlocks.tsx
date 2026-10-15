import { useEffect, useMemo, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { AlertTriangle, Check, ChevronRight, Circle, Clock3, File, Folder, Globe, Image as ImageIcon, Link2, MessageCircle, PauseCircle, Reply, Target, X, Zap } from 'lucide-react'
import { t } from '../i18n'
import type { Json } from '../types'
import { GroupAvatar } from './GroupAvatar'
import { MarkdownPreview } from './ArtifactPage'
import { orderedGroupMemberIds } from '../groupConversation.model.mjs'
import { getTaskCardStatus } from './messageBlocksTaskCard'
import { getSkillMessageDelivery, parseForcedSkillText, type LoadedSkillRecord, type SkillMessageDelivery } from './skillMessage.model'

export type ArtifactRef = { artifact_id: string; title: string; path_or_url: string; bot_id?: string; kind?: string }
export type MessageFileRef = { root: 'project' | 'bot' | 'upload'; root_id: string; path: string; name: string; size: number; mime: string }
export type MessageFilePreview = { content: string; content_encoding: 'utf8' | 'base64'; mime: string; size: number }

type ProjectData = { id?: string; chat_id?: string; name?: string; status?: string; done_at?: string | null; members?: { bot_id?: string; joined_at?: string }[] }
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

export function ProjectCard({ projectId, context, note, project, memberBotIds: memberIdsFallback = [], bots, handoff = false, onOpen }: { projectId: string; context?: string; note?: string | null; project?: ProjectData; memberBotIds?: string[]; bots: BotMap; handoff?: boolean; onOpen: (projectId: string, chatId?: string, completionHandoff?: boolean) => Promise<void> }) {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  if (!projectId) return <FallbackCard text={t('common.unsupported_block')} />
  const members = (project?.members ?? []).map((member) => bots[member.bot_id ?? '']?.name).filter((name): name is string => Boolean(name))
  const memberBotIds = orderedGroupMemberIds({ memberBotIds: memberIdsFallback, projectMembers: project?.members ?? [], bots }) as string[]
  const status = context === 'created' ? 'new' : project?.status ?? 'active'
  const statusText = t(status === 'new' ? 'project.status.new' : status === 'review' ? 'project.status.review' : status === 'done' ? 'project.status.done' : status === 'archived' ? 'project.status.archived' : 'project.status.active')
  const chatId = project?.chat_id ?? ''
  const heading = <div className="project-card-heading"><GroupAvatar memberBotIds={memberBotIds} bots={bots} size="40" label={project?.name ?? projectId} /><span className="project-card-main"><strong>{project?.name ?? projectId}</strong><small>{t('block.project_card.subtitle').replace('{members}', members.join('、'))}</small></span><span className={`project-status status-${status}`}>{statusText}</span><ChevronRight size={15} /></div>
  return <div className="project-card-wrap">{handoff
    ? <div className="project-card project-card-handoff">
      <div className="project-card-created">{t('block.project_card.acceptance')}</div>{heading}
      {note && <p className="project-card-note">{note}</p>}
      <button type="button" className="project-card-jump" disabled={busy} onClick={() => void open()}><MessageCircle size={13} />{t('block.project_card.jump_to_group')}<ChevronRight size={14} /></button>
    </div>
    : <button type="button" className="project-card" disabled={busy} onClick={() => void open()}>
      {context === 'created' && <div className="project-card-created">{t('block.project_card.created')}</div>}
      {heading}
      {note && <p className="project-card-note">{note}</p>}
      <div className="project-card-foot"><MessageCircle size={13} />{t('block.project_card.enter')}</div>
    </button>}{error && <p className="project-card-error" role="alert">{error}</p>}</div>

  async function open() {
    setBusy(true)
    setError('')
    try { await onOpen(projectId, chatId, handoff) } catch (reason) { setError(String(reason)) } finally { setBusy(false) }
  }
}

export function FallbackCard({ text, hint = t('block.fallback.hint') }: { text: string; hint?: string }) {
  return <div className="block-fallback"><AlertTriangle size={14} /><span>{text}<small>{hint}</small></span></div>
}

export function LoopPausedBlock({ state, hops, busy = false, onResolve }: { state: string; hops: number; busy?: boolean; onResolve: (action: 'continue' | 'end') => Promise<void> }) {
  const [working, setWorking] = useState(false)
  const [error, setError] = useState<string | null>(null)
  if (state !== 'paused') return <div className="protocol-status-block loop-result"><Check size={15} /><span>{t(state === 'continued' ? 'block.loop_paused.continued' : 'block.loop_paused.ended')}</span></div>
  const resolve = async (action: 'continue' | 'end') => {
    setWorking(true); setError(null)
    try { await onResolve(action) } catch (reason) { setError(String(reason)) } finally { setWorking(false) }
  }
  return <div className="protocol-status-block loop-paused"><PauseCircle size={15} /><span><strong>{t('block.loop_paused.title')}</strong><small>{t('block.loop_paused.desc').replace('{count}', String(hops))}</small>{error && <small role="alert">{error}</small>}</span><button type="button" disabled={busy || working} onClick={() => void resolve('continue')}>{t('block.loop_paused.continue')}</button><button type="button" className="secondary" disabled={busy || working} onClick={() => void resolve('end')}>{t('block.loop_paused.end')}</button></div>
}

export function TakeoverRequestBlock({ reason, state, onStart, onBack, onRelease }: { reason: string; state: string; onStart: () => void; onBack: () => void; onRelease: () => void }) {
  if (state === 'done') return <div className="protocol-status-block takeover-done"><Check size={15} /><span>{t('block.takeover.done')}</span></div>
  return <section className={`protocol-status-block takeover-${state}`}>
    <Target size={15} /><span><strong>{state === 'active' ? t('block.takeover.active') : t('block.takeover.title')}</strong>{reason && <small>{reason}</small>}</span>
    {state === 'active' ? <><button type="button" onClick={onBack}>{t('block.takeover.back_to_screen')}</button><button type="button" className="secondary" onClick={onRelease}>{t('block.takeover.release')}</button></> : <button type="button" onClick={onStart}>{t('block.takeover.start')}</button>}
  </section>
}

export function BotDmReferenceBlock({ senderName, recipientName, count, onOpen }: { senderName: string; recipientName: string; count: number; onOpen: () => void }) {
  return <button className="protocol-status-block bot-dm-reference" type="button" onClick={onOpen}><MessageCircle size={15} /><span>{t('block.bot_dm_ref.text').replace('{a}', senderName).replace('{b}', recipientName).replace('{count}', String(count))}</span><strong>{t('block.bot_dm_ref.expand')}</strong></button>
}

export function LinkPreviewBlock({ url, site, title, description, image, onReadFile }: { url: string; site: string; title: string; description?: string | null; image?: MessageFileRef | null; onReadFile: (file: MessageFileRef) => Promise<MessageFilePreview> }) {
  return <a className="protocol-link-preview" href={url} target="_blank" rel="noreferrer">
    <span className="protocol-link-copy"><small>{site}</small><strong>{title || url}</strong>{description && <span>{description}</span>}<code><Link2 size={11} />{url}</code></span>{image && <span className="protocol-link-preview-thumb" title={image.name}><FileImagePreview file={image} onReadFile={onReadFile} /></span>}<ChevronRight size={15} />
  </a>
}

export function BlockedBlock({ reason, mainBotName, canRetry, onRetry, onSwitchModel }: { reason: string; mainBotName: string; canRetry: boolean; onRetry: () => void; onSwitchModel: () => void }) {
  return <section className="protocol-blocked"><header><AlertTriangle size={15} /><strong>{t('block.blocked.title')}</strong></header><p>{reason}</p><small>{t('block.blocked.need').replace('{who}', mainBotName)}</small><footer><button type="button" disabled={!canRetry} onClick={onRetry}>{t('common.retry')}</button><button type="button" onClick={onSwitchModel}>{t('block.blocked.switch_model')}</button></footer></section>
}

export function FileBlock({ file, image = false }: { file: Record<string, Json>; image?: boolean }) {
  const name = String(file.name ?? file.path ?? '')
  const size = Number(file.size ?? 0)
  return <div className={`protocol-file-block ${image ? 'is-image' : ''}`}>{image ? <ImageIcon size={18} /> : <File size={18} />}<span><strong>{name}</strong>{!image && size > 0 && <small>{formatBytes(size)}</small>}</span></div>
}

export function MessageImageBlock({ file, onReadFile }: { file: MessageFileRef | null; onReadFile: (file: MessageFileRef) => Promise<MessageFilePreview> }) {
  if (!file) return <div className="protocol-file-block is-image"><ImageIcon size={18} /><span><strong>{t('common.unsupported_block')}</strong></span></div>
  return <ImageFileView file={file} onReadFile={onReadFile} />
}

function FileImagePreview({ file, onReadFile }: { file: MessageFileRef; onReadFile: (file: MessageFileRef) => Promise<MessageFilePreview> }) {
  const target = useRef<HTMLSpanElement>(null)
  const visible = useNearViewport(target)
  const { src, error, fail } = useFileImage(file, onReadFile, visible)
  return <span className="protocol-link-image-inner" ref={target}>{src ? <img src={src} alt={file.name} loading="lazy" onError={fail} /> : <><File size={15} /><small title={error ?? undefined}>{file.name}</small>{error && <small>{t('block.image.unavailable')}</small>}</>}</span>
}

function ImageFileView({ file, onReadFile }: { file: MessageFileRef; onReadFile: (file: MessageFileRef) => Promise<MessageFilePreview> }) {
  const target = useRef<HTMLDivElement>(null)
  const trigger = useRef<HTMLButtonElement>(null)
  const closeButton = useRef<HTMLButtonElement>(null)
  const visible = useNearViewport(target)
  const { src, error, loading, retry, fail } = useFileImage(file, onReadFile, visible)
  const [open, setOpen] = useState(false)
  useEffect(() => {
    if (!open) return
    const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null
    const triggerButton = trigger.current
    closeButton.current?.focus()
    const closeOnEscape = (event: KeyboardEvent) => { if (event.key === 'Escape') setOpen(false) }
    window.addEventListener('keydown', closeOnEscape)
    return () => {
      window.removeEventListener('keydown', closeOnEscape)
      if (previousFocus?.isConnected) previousFocus.focus()
      else triggerButton?.focus()
    }
  }, [open])
  return <div className="protocol-image-wrap" ref={target}>
    {src ? <button ref={trigger} type="button" className="protocol-image-button" onClick={() => setOpen(true)} aria-label={file.name}><img src={src} alt={file.name} onError={fail} /></button>
      : loading ? <div className="protocol-image-loading"><ImageIcon size={18} /><span>{t('block.image.loading')}</span></div>
        : <button type="button" className="protocol-file-block is-image protocol-image-fallback" title={error ?? undefined} onClick={retry}><ImageIcon size={18} /><span><strong>{file.name}</strong><small>{t('block.image.unavailable')}</small></span></button>}
    {open && src && createPortal(<div className="protocol-image-viewer" role="dialog" aria-modal="true" aria-label={file.name} onMouseDown={(event) => { if (event.target === event.currentTarget) setOpen(false) }}><button ref={closeButton} type="button" className="protocol-image-close" onClick={() => setOpen(false)}>{t('common.close')}</button><img src={src} alt={file.name} onError={fail} /></div>, document.body)}
  </div>
}

function useNearViewport(target: { current: Element | null }) {
  const [visible, setVisible] = useState(false)
  useEffect(() => {
    const node = target.current
    if (!node || typeof IntersectionObserver === 'undefined') { setVisible(true); return }
    const observer = new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting)) {
        setVisible(true)
        observer.disconnect()
      }
    }, { rootMargin: '160px' })
    observer.observe(node)
    return () => observer.disconnect()
  }, [target])
  return visible
}

function useFileImage(file: MessageFileRef, onReadFile: (file: MessageFileRef) => Promise<MessageFilePreview>, enabled: boolean) {
  const [src, setSrc] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)
  const [retryCount, setRetryCount] = useState(0)
  const stableFile = useMemo(() => ({
    root: file.root,
    root_id: file.root_id,
    path: file.path,
    name: file.name,
    size: file.size,
    mime: file.mime,
  }), [file.root, file.root_id, file.path, file.name, file.size, file.mime])
  useEffect(() => {
    if (!enabled) return
    let active = true
    setSrc(null)
    setError(null)
    setLoading(true)
    void onReadFile(stableFile).then((preview) => {
      const mime = preview.mime.split(';', 1)[0].trim().toLowerCase()
      if (preview.content_encoding !== 'base64' || !/^image\/(png|jpeg|gif|webp|avif)$/i.test(mime) || !Number.isFinite(preview.size) || preview.size > 5 * 1024 * 1024) throw new Error('unsupported image preview')
      if (active) setSrc(`data:${mime};base64,${preview.content}`)
    }).catch((reason: unknown) => {
      if (active) setError(String(reason))
    }).finally(() => {
      if (active) setLoading(false)
    })
    return () => { active = false }
  }, [stableFile, onReadFile, retryCount, enabled])
  return { src, error, loading, retry: () => setRetryCount((value) => value + 1), fail: () => { setSrc(null); setError('image decode failed') } }
}

export function TaskCard({ assignment, groupName, botName, onDetail, row = false }: { assignment?: Record<string, Json>; groupName?: string; botName?: string; onDetail: (id: string) => void; row?: boolean }) {
  if (!assignment) return null
  const id = String(assignment.id ?? '')
  const title = String(assignment.title ?? assignment.bot_id ?? '')
  const status = String(assignment.status ?? 'working')
  const { statusKey, recovered } = getTaskCardStatus(status, assignment.recovered_at)
  const elapsed = formatDuration(assignment.started_at ?? assignment.created_at, assignment.finished_at)
  const elapsedValue = durationValue(assignment.started_at ?? assignment.created_at, assignment.finished_at)
  const reason = status === 'queued' && assignment.queue_reason ? t('block.task_card.queued_reason').replace('{task}', String(assignment.queue_reason)) : typeof assignment.failure_reason === 'string' ? assignment.failure_reason : ''
  if (status === 'done') return <div className={`task-card-done-line${row ? ' task-card-done-row' : ''}`}><Check size={14} /><span>{t('block.task_card.done_line').replace('{bot}', botName ?? t('common.bot')).replace('{title}', title).replace('{duration}', elapsedValue || '—')}{recovered && ` · ${t('status.recovered')}`}</span><button type="button" className="task-detail-button" onClick={() => id && onDetail(id)}>{t('common.details')}</button></div>
  return <div className={`task-card task-${status}${row ? ' task-card-in-row' : ''}`}>
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
    <div className="completion-heading"><Check size={14} /><strong>{t('block.completion.done')}</strong></div>
    {summary && <div className="completion-summary"><MarkdownPreview content={summary} variant="message" /></div>}
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
    const externalUrl = artifact.kind === 'url' && /^https?:\/\//i.test(artifact.path_or_url)
    return <div className="artifact-row" key={artifact.artifact_id}>
      <span className="artifact-icon"><Icon size={16} /></span><span className="artifact-meta"><span className="artifact-title">{artifact.title}<small>{bots[artifact.bot_id ?? '']?.name ?? ''}</small></span><small className="artifact-path">{artifact.path_or_url}</small></span>
      {!compact && (externalUrl ? <a className="artifact-preview" href={artifact.path_or_url} target="_blank" rel="noreferrer">{t('artifact.preview.open_url')}</a> : <button type="button" className="artifact-preview" onClick={() => onPreview(artifact)}>{t('artifact.preview.tabs.preview')}</button>)}
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
function formatBytes(value: number) {
  if (!Number.isFinite(value) || value < 0) return ''
  if (value < 1024) return `${value} B`
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`
  return `${(value / (1024 * 1024)).toFixed(1)} MB`
}
function formatTime(value?: string | null) {
  if (!value) return ''
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? '' : date.toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' })
}
function statusText(key: string) { return t(key as Parameters<typeof t>[0]) }
