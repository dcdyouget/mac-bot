import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { AlertCircle, Check, ChevronLeft, ChevronRight, Clock3, Pencil, Plus, Search, Sparkles, Trash2, Undo2 } from 'lucide-react'
import { t } from '../i18n'
import { applySavedProfileField, flushUserProfileSaveQueue, reconcileUserProfileConflict, type UserProfileConflict } from './userProfileConflict'
import { isMemoryQuotaAtLimit, shouldSaveOnEnter } from './memoryPageModel'
import './MemoryPage.css'

export type MemorySender = { kind: 'user' | 'system' } | { kind: 'bot'; bot_id: string }
export type UserMemoryEntry = {
  id: string
  content: string
  version: number
  source: { by: MemorySender; chat_id: string | null; message_id: string | null }
  updated_by: MemorySender
  created_at: string
  updated_at: string
}
export type UserMemoryQuota = { used_chars: number; limit_chars: number }
export type UserProfile = {
  name: string | null
  language: string | null
  identity: string | null
  timezone: string
  onboarded: boolean
  version: number
  updated_by: MemorySender
  updated_at: string
}
export type UserProfilePatch = Partial<Pick<UserProfile, 'name' | 'language' | 'identity'>>
export type MemoryPageProps = {
  profile: UserProfile | null
  entries: UserMemoryEntry[]
  quota: UserMemoryQuota | null
  chats?: Record<string, string>
  bots?: Record<string, string>
  loading?: boolean
  loadError?: string | null
  onReload: () => Promise<void>
  onUpdateProfile: (patch: UserProfilePatch, expectedVersion: number) => Promise<UserProfile>
  onCreate: (content: string) => Promise<{ entry: UserMemoryEntry; quota: UserMemoryQuota }>
  onUpdate: (entryId: string, content: string, expectedVersion: number) => Promise<{ entry: UserMemoryEntry; quota: UserMemoryQuota }>
  onDelete: (entryId: string, expectedVersion: number) => Promise<{ quota: UserMemoryQuota }>
  onOpenSource?: (chatId: string, messageId: string | null) => Promise<void>
  onBack?: () => void
}

type EditableProfileField = 'name' | 'language' | 'identity'
type ProfileForm = Pick<UserProfile, EditableProfileField>
type ParsedError = { code?: string; current?: UserMemoryEntry | UserProfile; message: string }

const charCount = (text: string) => [...text].length
const formatDate = (value: string) => {
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return value
  const parts = new Intl.DateTimeFormat('en-US', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', hourCycle: 'h23' }).formatToParts(date)
  const part = (type: Intl.DateTimeFormatPartTypes) => parts.find((item) => item.type === type)?.value ?? ''
  return `${part('month')}/${part('day')} ${part('hour')}:${part('minute')}`
}
const senderKind = (sender: MemorySender) => sender?.kind ?? 'user'
const isProfile = (value: UserMemoryEntry | UserProfile | undefined): value is UserProfile => Boolean(value && 'timezone' in value && 'name' in value)

function parseError(reason: unknown): ParsedError {
  let value: unknown = reason
  if (typeof value === 'string') {
    try { value = JSON.parse(value) as unknown } catch { /* Keep the original text for user feedback. */ }
  }
  if (value && typeof value === 'object') {
    const record = value as Record<string, unknown>
    const details = record.details && typeof record.details === 'object' ? record.details as Record<string, unknown> : undefined
    const current = details?.current as UserMemoryEntry | UserProfile | undefined
    const code = typeof record.code === 'string' ? record.code : undefined
    const message = typeof record.message === 'string' ? record.message : String(reason)
    return { code, current, message }
  }
  const message = String(reason)
  const code = message.includes('quota_full') ? 'quota_full' : message.includes('conflict') ? 'conflict' : undefined
  return { code, message }
}

export function MemoryPage(props: MemoryPageProps) {
  const { profile, entries, quota, chats = {}, bots = {}, loading = false, loadError, onReload, onUpdateProfile, onCreate, onUpdate, onDelete, onOpenSource, onBack } = props
  const [profileForm, setProfileForm] = useState<ProfileForm>({ name: profile?.name ?? null, language: profile?.language ?? null, identity: profile?.identity ?? null })
  const [profileSaved, setProfileSaved] = useState<EditableProfileField | null>(null)
  const [profileConflict, setProfileConflict] = useState<UserProfileConflict | null>(null)
  const [profileError, setProfileError] = useState<string | null>(null)
  const [query, setQuery] = useState('')
  const [editingId, setEditingId] = useState<string | null>(null)
  const [editContent, setEditContent] = useState('')
  const [editBase, setEditBase] = useState<UserMemoryEntry | null>(null)
  const [newEntry, setNewEntry] = useState(false)
  const [entryConflict, setEntryConflict] = useState<{ current: UserMemoryEntry; mine: string; operation: 'update' | 'delete'; open: boolean } | null>(null)
  const [quotaFull, setQuotaFull] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [undoEntry, setUndoEntry] = useState<UserMemoryEntry | null>(null)
  const [localQuota, setLocalQuota] = useState(quota)
  const undoTimer = useRef<number | null>(null)
  const profileSnapshot = useRef<UserProfile | null>(null)
  const profileFormRef = useRef<ProfileForm>({ name: profile?.name ?? null, language: profile?.language ?? null, identity: profile?.identity ?? null })
  const profileSaveLock = useRef(false)
  const pendingProfileSaves = useRef<Partial<ProfileForm>>({})
  const profileReloadRequested = useRef(false)
  const [profileReloading, setProfileReloading] = useState(false)
  const entryOverrides = useRef<Record<string, UserMemoryEntry>>({})
  const hiddenEntries = useRef<Set<string>>(new Set())
  const [localEntries, setLocalEntries] = useState<UserMemoryEntry[]>(entries)

  useEffect(() => {
    setLocalEntries(() => {
      const merged = new Map(entries.map((entry) => [entry.id, entry]))
      for (const [id, entry] of Object.entries(entryOverrides.current)) {
        const incoming = merged.get(id)
        if (!incoming || incoming.version < entry.version) merged.set(id, entry)
        else delete entryOverrides.current[id]
      }
      for (const id of hiddenEntries.current) merged.delete(id)
      return [...merged.values()].sort((a, b) => b.updated_at.localeCompare(a.updated_at))
    })
  }, [entries])

  useEffect(() => {
    if (!profile) return
    const previous = profileSnapshot.current
    const currentForm = profileFormRef.current
    profileSnapshot.current = profile
    if (profileReloadRequested.current || !previous) {
      profileReloadRequested.current = false
      const next = { name: profile.name, language: profile.language, identity: profile.identity }
      profileFormRef.current = next
      setProfileForm(next)
      setProfileConflict(null)
      setProfileError(null)
      setProfileReloading(false)
      return
    }
    setProfileConflict((current) => reconcileUserProfileConflict(previous, profile, currentForm, current))
    const fields = Object.keys(currentForm) as EditableProfileField[]
    const next = Object.fromEntries(fields.map((key) => [
      key,
      currentForm[key] !== previous[key] ? currentForm[key] : profile[key],
    ])) as ProfileForm
    profileFormRef.current = next
    setProfileForm(next)
  }, [profile])

  useEffect(() => () => { if (undoTimer.current !== null) window.clearTimeout(undoTimer.current) }, [])
  useEffect(() => { setLocalQuota(quota) }, [quota])

  useEffect(() => {
    if (!editingId || !editBase) return
    const latest = entries.find((entry) => entry.id === editingId)
    if (latest && latest.version !== editBase.version && latest.content !== editBase.content) {
      setEntryConflict((current) => current ?? { current: latest, mine: editContent, operation: 'update', open: false })
    }
  }, [entries, editingId, editBase, editContent])

  const filteredEntries = useMemo(() => localEntries
    .filter((entry) => entry.content.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()))
    .sort((a, b) => b.updated_at.localeCompare(a.updated_at)), [localEntries, query])
  const quotaPercent = localQuota && localQuota.limit_chars > 0 ? Math.min(100, localQuota.used_chars / localQuota.limit_chars * 100) : 0
  const quotaAtLimit = isMemoryQuotaAtLimit(localQuota)

  async function run(action: () => Promise<void>, conflictContext?: { operation: 'update' | 'delete'; mine: string }) {
    setBusy(true)
    setError(null)
    try { await action() } catch (reason) {
      const parsed = parseError(reason)
      if (parsed.code === 'quota_full') setQuotaFull(true)
      else if (parsed.code === 'conflict' && !isProfile(parsed.current) && parsed.current) setEntryConflict({ current: parsed.current, mine: conflictContext?.mine ?? editContent, operation: conflictContext?.operation ?? 'update', open: false })
      else setError(parsed.message)
    } finally { setBusy(false) }
  }

  async function saveProfile(field: EditableProfileField, nextValue?: string | null) {
    if (!profile || profileConflict) return
    const value = nextValue === undefined ? profileFormRef.current[field] : nextValue
    pendingProfileSaves.current[field] = value
    if (profileSaveLock.current) return
    profileSaveLock.current = true
    try {
      setProfileError(null)
      await flushUserProfileSaveQueue(
        pendingProfileSaves.current,
        () => profileSnapshot.current ?? profile,
        onUpdateProfile,
        () => { setProfileSaved(null) },
        (pendingField, pendingValue, updated) => {
          const freshest = profileSnapshot.current
          if (!freshest || updated.version >= freshest.version) profileSnapshot.current = updated
          const savedProfile = profileSnapshot.current ?? updated
          const next = applySavedProfileField(profileFormRef.current, pendingField, pendingValue, savedProfile)
          profileFormRef.current = next
          setProfileForm(next)
          setProfileSaved(pendingField)
          window.setTimeout(() => setProfileSaved((saved) => saved === pendingField ? null : saved), 1800)
        },
        (_pendingField, patch, reason) => {
          const parsed = parseError(reason)
          if (parsed.code === 'conflict' && isProfile(parsed.current)) setProfileConflict({ current: parsed.current, mine: patch, open: false })
          else setProfileError(parsed.code === 'quota_full' ? t('memory.quota_full') : parsed.message)
        },
      )
    } finally {
      profileSaveLock.current = false
    }
  }

  function updateProfileField(field: EditableProfileField, value: string | null) {
    const next = { ...profileFormRef.current, [field]: value }
    profileFormRef.current = next
    setProfileForm(next)
  }

  async function reloadProfile() {
    profileReloadRequested.current = true
    setProfileReloading(true)
    try {
      await onReload()
    } catch {
      profileReloadRequested.current = false
    } finally {
      setProfileReloading(false)
    }
  }

  async function saveEntry() {
    const content = editContent.trim()
    if (!content || charCount(content) > 500) return
    await run(async () => {
      if (newEntry) {
        const result = await onCreate(content)
        setLocalQuota(result.quota)
        setQuotaFull(false)
        entryOverrides.current[result.entry.id] = result.entry
        hiddenEntries.current.delete(result.entry.id)
        setLocalEntries((current) => [result.entry, ...current.filter((entry) => entry.id !== result.entry.id)])
      } else if (editBase) {
        const result = await onUpdate(editBase.id, content, editBase.version)
        setLocalQuota(result.quota)
        setQuotaFull(false)
        entryOverrides.current[result.entry.id] = result.entry
        hiddenEntries.current.delete(result.entry.id)
        setLocalEntries((current) => current.map((entry) => entry.id === result.entry.id ? result.entry : entry).sort((a, b) => b.updated_at.localeCompare(a.updated_at)))
      }
      cancelEdit()
    }, { operation: 'update', mine: content })
  }

  function cancelEdit() {
    setEditingId(null)
    setEditContent('')
    setEditBase(null)
    setNewEntry(false)
    setEntryConflict(null)
  }

  function beginEdit(entry: UserMemoryEntry) {
    setEditingId(entry.id)
    setEditContent(entry.content)
    setEditBase(entry)
    setNewEntry(false)
    setEntryConflict(null)
    setError(null)
  }

  function beginCreate() {
    setEditingId('new')
    setEditContent('')
    setEditBase(null)
    setNewEntry(true)
    setEntryConflict(null)
    setError(null)
  }

  async function deleteEntry(entry: UserMemoryEntry) {
    await run(async () => {
      const result = await onDelete(entry.id, entry.version)
      setLocalQuota(result.quota)
      setQuotaFull(false)
      hiddenEntries.current.add(entry.id)
      entryOverrides.current[entry.id] = entry
      setLocalEntries((current) => current.filter((item) => item.id !== entry.id))
      setUndoEntry(entry)
      if (undoTimer.current !== null) window.clearTimeout(undoTimer.current)
      undoTimer.current = window.setTimeout(() => setUndoEntry(null), 5000)
    }, { operation: 'delete', mine: entry.content })
  }

  async function undoDelete() {
    if (!undoEntry) return
    await run(async () => {
      const result = await onCreate(undoEntry.content)
      setLocalQuota(result.quota)
      setQuotaFull(false)
      entryOverrides.current[result.entry.id] = result.entry
      hiddenEntries.current.delete(result.entry.id)
      setLocalEntries((current) => [result.entry, ...current.filter((entry) => entry.id !== result.entry.id)].sort((a, b) => b.updated_at.localeCompare(a.updated_at)))
      setUndoEntry(null)
      if (undoTimer.current !== null) window.clearTimeout(undoTimer.current)
    })
  }

  function chooseTheirs() {
    if (!entryConflict) return
    const current = entryConflict.current
    entryOverrides.current[current.id] = current
    setLocalEntries((items) => items.map((entry) => entry.id === current.id ? current : entry))
    cancelEdit()
  }

  async function keepMine() {
    if (!entryConflict) return
    const { current, mine, operation } = entryConflict
    await run(async () => {
      if (operation === 'delete') {
        const result = await onDelete(current.id, current.version)
        setLocalQuota(result.quota)
        setQuotaFull(false)
        hiddenEntries.current.add(current.id)
        entryOverrides.current[current.id] = current
        setLocalEntries((items) => items.filter((entry) => entry.id !== current.id))
        setUndoEntry(current)
        if (undoTimer.current !== null) window.clearTimeout(undoTimer.current)
        undoTimer.current = window.setTimeout(() => setUndoEntry(null), 5000)
      } else {
        const result = await onUpdate(current.id, mine.trim(), current.version)
        setLocalQuota(result.quota)
        setQuotaFull(false)
        entryOverrides.current[result.entry.id] = result.entry
        setLocalEntries((items) => items.map((entry) => entry.id === result.entry.id ? result.entry : entry))
      }
      cancelEdit()
    }, { operation, mine })
  }

  return <section className="memory-page">
    <header className="memory-page-toolbar">{onBack && <button type="button" onClick={onBack}><ChevronLeft size={15} />{t('nav.settings')}</button>}<strong>{t('settings.section.memory')}</strong></header>
    <div className="memory-scroll">
      {loadError && <div className="memory-error" role="alert"><AlertCircle size={14} /><span>{loadError}</span><button type="button" onClick={() => void onReload()}>{t('common.retry')}</button></div>}
      {error && <div className="memory-error" role="alert"><AlertCircle size={14} /><span>{error}</span></div>}
      <div className="memory-content">
        <p className="memory-description">{t('memory.desc')}</p>

        {profileConflict && <div className="memory-conflict-banner memory-profile-conflict" role="alert"><AlertCircle size={15} /><button type="button" disabled={profileReloading} title={t('memory.profile.conflict_reload')} onClick={() => void reloadProfile()}>{t('memory.profile.conflict_banner').replace('{version}', String(profileConflict.current.version))}</button></div>}

        <section className="memory-card memory-profile-card">
          <h2>{t('memory.profile.title')}</h2>
          {!profile && (loading
            ? <div className="memory-profile-skeleton" role="status" aria-label={t('memory.page.profile_loading')}><span /><span /><span /><span /></div>
            : <div className="memory-loading">{t('memory.page.profile_empty')}</div>)}
          {profile && <>
            {profileError && <div className="memory-inline-error">{profileError}</div>}
            <div className="memory-profile-fields">
              <label><span>{t('memory.profile.name')}</span><input disabled={profileReloading} maxLength={20} placeholder={t('memory.profile.name_placeholder')} value={profileForm.name ?? ''} onChange={(event) => updateProfileField('name', event.target.value || null)} onBlur={() => void saveProfile('name')} onKeyDown={(event) => { if (shouldSaveOnEnter({ key: event.key, isComposing: event.nativeEvent.isComposing, keyCode: event.keyCode })) { event.preventDefault(); void saveProfile('name') } }} />{profileSaved === 'name' && <Check className="memory-saved" size={14} aria-label={t('memory.profile.save_success')} />}</label>
              <label><span>{t('memory.profile.language')}</span><select disabled={profileReloading} value={profileForm.language ?? ''} onChange={(event) => { const language = event.target.value || null; updateProfileField('language', language); void saveProfile('language', language) }}><option value="">{t('memory.profile.language.system_option')}</option><option value="zh-CN">{t('memory.profile.language.zh_cn')}</option><option value="en-US">{t('memory.profile.language.en_us')}</option><option value="ja-JP">{t('memory.profile.language.ja_jp')}</option><option value="ko-KR">{t('memory.profile.language.ko_kr')}</option>{profileForm.language && !['zh-CN', 'en-US', 'ja-JP', 'ko-KR'].includes(profileForm.language) && <option value={profileForm.language}>{profileForm.language}</option>}</select>{profileSaved === 'language' && <Check className="memory-saved" size={14} aria-label={t('memory.profile.save_success')} />}</label>
              <label><span>{t('memory.profile.identity')}</span><input disabled={profileReloading} maxLength={100} placeholder={t('memory.profile.identity_placeholder')} value={profileForm.identity ?? ''} onChange={(event) => updateProfileField('identity', event.target.value || null)} onBlur={() => void saveProfile('identity')} onKeyDown={(event) => { if (shouldSaveOnEnter({ key: event.key, isComposing: event.nativeEvent.isComposing, keyCode: event.keyCode })) { event.preventDefault(); void saveProfile('identity') } }} />{profileSaved === 'identity' && <Check className="memory-saved" size={14} aria-label={t('memory.profile.save_success')} />}</label>
              <div className="memory-timezone"><span>{t('memory.profile.timezone')}</span><span className="memory-readonly"><Clock3 size={13} />{t('memory.profile.timezone_readonly').replace('{tz}', profile.timezone)}</span></div>
            </div>
          </>}
        </section>

        <section className="memory-card memory-usage-card">
          <div className="memory-usage-heading"><strong>{t('memory.usage')}</strong><code>{t('memory.usage_value').replace('{used}', (localQuota?.used_chars ?? 0).toLocaleString()).replace('{limit}', (localQuota?.limit_chars ?? 1500).toLocaleString())}</code></div>
          <div className="memory-progress"><span className={quotaPercent >= 100 ? 'full' : quotaPercent >= 90 ? 'near' : ''} style={{ width: `${quotaPercent}%` }} /></div>
          <p>{t('memory.quota.warning_text')}</p>
        </section>

        <div className="memory-tools"><label className="memory-search"><Search size={14} /><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder={t('memory.search')} /></label><button className="memory-button memory-button-primary" type="button" onClick={beginCreate}><Plus size={14} />{t('memory.add')}</button></div>
        {(quotaFull || quotaAtLimit) && <div className="memory-quota-error" role="alert">{t('memory.quota_full')}</div>}
        {entryConflict && <div className="memory-conflict-banner"><AlertCircle size={15} /><span>{t('memory.conflict').replace('{who}', senderName(entryConflict.current.updated_by, bots)).replace('{n}', String(entryConflict.current.version))}</span><button type="button" onClick={() => setEntryConflict((current) => current ? { ...current, open: !current.open } : current)}>{t('memory.conflict.view_versions')}</button></div>}
        {entryConflict?.open && <div className="memory-compare"><div><small>{t('memory.conflict.mine_label')}</small><p>{entryConflict.operation === 'delete' ? t('memory.delete_entry') : entryConflict.mine}</p></div><div><small>{t('memory.conflict.theirs_label')}</small><p>{entryConflict.current.content}</p></div><footer><button type="button" onClick={chooseTheirs}>{t('memory.conflict.theirs')}</button><button type="button" className="memory-button-primary" disabled={busy} onClick={() => void keepMine()}>{t('memory.conflict.mine')}</button></footer></div>}
        <div className="memory-list" aria-busy={loading}>
          {newEntry && <MemoryEditor content={editContent} busy={busy} onChange={setEditContent} onSave={() => void saveEntry()} onCancel={cancelEdit} />}
          {loading && !localEntries.length && <div className="memory-list-state">{t('memory.page.loading')}</div>}
          {!loading && !filteredEntries.length && !newEntry && <div className="memory-list-state">{query ? t('memory.page.search_empty') : t('memory.page.empty')}</div>}
          {filteredEntries.map((entry) => editingId === entry.id ? <MemoryEditor key={entry.id} content={editContent} busy={busy} onChange={setEditContent} onSave={() => void saveEntry()} onCancel={cancelEdit} /> : <MemoryRow key={entry.id} entry={entry} bots={bots} chatTitle={entry.source.chat_id ? chats[entry.source.chat_id] : undefined} query={query} onEdit={() => beginEdit(entry)} onDelete={() => void deleteEntry(entry)} onOpenSource={onOpenSource} />)}
        </div>
      </div>
    </div>
    {undoEntry && <div className="memory-toast" role="status"><Check size={14} /><span>{t('memory.deleted')}</span><button type="button" onClick={() => void undoDelete()} disabled={busy}><Undo2 size={13} />{t('memory.undo')}</button></div>}
  </section>
}

function senderName(sender: MemorySender, bots: Record<string, string>) {
  if (sender.kind === 'system') return t('memory.source.system_name')
  if (sender.kind === 'bot') return bots[sender.bot_id] ?? t('common.bot')
  return t('common.you')
}

function highlight(content: string, query: string) {
  const term = query.trim()
  if (!term) return content
  const lower = content.toLocaleLowerCase()
  const needle = term.toLocaleLowerCase()
  const parts: ReactNode[] = []
  let offset = 0
  let index = lower.indexOf(needle)
  while (index >= 0) {
    if (index > offset) parts.push(content.slice(offset, index))
    parts.push(<mark key={`${index}-${offset}`}>{content.slice(index, index + term.length)}</mark>)
    offset = index + term.length
    index = lower.indexOf(needle, offset)
  }
  if (offset === 0) return content
  if (offset < content.length) parts.push(content.slice(offset))
  return parts
}

function MemoryRow({ entry, bots, chatTitle, query, onEdit, onDelete, onOpenSource }: { entry: UserMemoryEntry; bots: Record<string, string>; chatTitle?: string; query: string; onEdit: () => void; onDelete: () => void; onOpenSource?: (chatId: string, messageId: string | null) => Promise<void> }) {
  const by = senderKind(entry.source.by)
  const sourceChatTitle = chatTitle ?? t('memory.page.open_source')
  return <article className="memory-row">
    <div className="memory-row-main"><button className="memory-entry-content" type="button" onClick={onEdit} aria-label={t('memory.edit_entry')}>{highlight(entry.content, query)}</button><div className="memory-source-line">
      {by === 'system' ? <><Sparkles size={16} /><span>{t('memory.source.curator')}</span>{entry.source.chat_id && <><span>·</span><button type="button" className="memory-source-link" onClick={() => void onOpenSource?.(entry.source.chat_id!, entry.source.message_id)}>{sourceChatTitle}<ChevronRight size={12} /></button></>}</>
        : by === 'bot' ? <><span className="memory-bot-avatar">{senderName(entry.source.by, bots).slice(0, 1)}</span><span>{t('memory.source.bot').replace('{name}', senderName(entry.source.by, bots))}</span>{entry.source.chat_id && <><span>·</span><button type="button" className="memory-source-link" onClick={() => void onOpenSource?.(entry.source.chat_id!, entry.source.message_id)}>{sourceChatTitle}<ChevronRight size={12} /></button></>}</>
          : <span>{t('memory.source.user')}</span>}
      <span>·</span><time dateTime={entry.updated_at}>{formatDate(entry.updated_at)}</time>
    </div></div>
    <div className="memory-row-actions"><button type="button" aria-label={t('memory.edit_entry')} title={t('memory.edit_entry')} onClick={onEdit}><Pencil size={14} /></button><button type="button" aria-label={t('memory.delete_entry')} title={t('memory.delete_entry')} onClick={onDelete}><Trash2 size={14} /></button></div>
  </article>
}

function MemoryEditor({ content, busy, onChange, onSave, onCancel }: { content: string; busy: boolean; onChange: (value: string) => void; onSave: () => void; onCancel: () => void }) {
  return <form className="memory-editor" onSubmit={(event) => { event.preventDefault(); onSave() }} onKeyDown={(event) => { if (event.key === 'Escape') { event.preventDefault(); onCancel() } else if ((event.metaKey || event.ctrlKey) && shouldSaveOnEnter({ key: event.key, isComposing: event.nativeEvent.isComposing, keyCode: event.keyCode })) { event.preventDefault(); onSave() } }}>
    <textarea autoFocus maxLength={500} value={content} onChange={(event) => onChange(event.target.value)} aria-label={t('memory.editor.content_label')} />
    <div className="memory-editor-footer"><span>{t('memory.edit.count').replace('{count}', String(charCount(content)))}</span><div><button type="button" disabled={busy} onClick={onCancel}>{t('common.cancel')}</button><button type="submit" className="memory-button-primary" disabled={busy || !content.trim() || charCount(content) > 500}>{busy ? t('memory.editor.saving') : t('memory.editor.save')}</button></div></div>
    {charCount(content) > 500 && <small className="memory-inline-error">{t('memory.editor.max_length')}</small>}
  </form>
}
